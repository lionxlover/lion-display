//! Backlight policy — sysfs brightness with clamped ramps.
//!
//! Real backlight devices live under `/sys/class/backlight/<name>/`
//! with `brightness`, `max_brightness`, `bl_power`, and `type` files.
//! The policy here is pure logic over a [`BacklightFs`] seam: reading
//! and parsing the files, clamping writes into `[0, max]`, and walking
//! deterministic integer ramps between levels (fade-in/fade-out), so the
//! DPMS-on fade never pops and never overshoots. The real filesystem
//! implementation is [`SysFs`]; tests drive [`MemFs`].
//!
//! Driver kinds matter for power policy (`ldp-power`, Phase 16): `raw`
//! panels need platform-specific minimums, `firmware` ones are
//! self-managing. The parsed kind is carried through untouched.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{DisplayError, Result};

/// The backlight driver kind (`/sys/class/backlight/*/type`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub enum BacklightKind {
    /// Platform-raw register control (needs minimum clamping).
    Raw,
    /// Firmware/EC-managed (self-terminating ramps).
    Firmware,
    /// Platform driver with its own power logic.
    Platform,
    /// GPU-internal panel control.
    Panel,
    /// Unknown/missing type file.
    #[default]
    Unknown,
}

impl BacklightKind {
    /// Parse the sysfs `type` string.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "raw" => Self::Raw,
            "firmware" => Self::Firmware,
            "platform" => Self::Platform,
            "panel" => Self::Panel,
            _ => Self::Unknown,
        }
    }

    /// The hard floor for brightness on this kind — raw panels cannot go
    /// below the platform minimum without risking controller damage;
    /// firmware-managed ones clamp themselves.
    #[must_use]
    pub const fn brightness_floor(self) -> u64 {
        match self {
            Self::Raw => 1,
            _ => 0,
        }
    }
}

/// Filesystem seam for `/sys/class/backlight`.
pub trait BacklightFs {
    /// Read one file to a trimmed string; missing files are `Ok(None)`.
    ///
    /// # Errors
    /// Implementation-defined I/O failures (the sysfs path exists but
    /// the read failed).
    fn read(&self, path: &Path) -> Result<Option<String>>;
    /// Write one file.
    ///
    /// # Errors
    /// Implementation-defined I/O failures.
    fn write(&self, path: &Path, value: &str) -> Result<()>;
}

/// The real sysfs.
#[derive(Clone, Copy, Debug, Default)]
pub struct SysFs;

impl BacklightFs for SysFs {
    fn read(&self, path: &Path) -> Result<Option<String>> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(Some(s.trim().to_owned())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(DisplayError::BacklightIo {
                path: path.to_owned(),
                errno: e.raw_os_error().unwrap_or(-1),
            }),
        }
    }

    fn write(&self, path: &Path, value: &str) -> Result<()> {
        std::fs::write(path, value).map_err(|e| DisplayError::BacklightIo {
            path: path.to_owned(),
            errno: e.raw_os_error().unwrap_or(-1),
        })
    }
}

/// One backlight device, bound to a seam.
pub struct BacklightDevice {
    fs: std::sync::Arc<dyn BacklightFs + Send + Sync>,
    root: PathBuf,
    name: String,
    kind: BacklightKind,
    max: u64,
    /// Floors combine the driver kind's minimum with any platform value
    /// discovered at construction (kept even if files vanish later).
    floor: u64,
}

impl core::fmt::Debug for BacklightDevice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The seam is deliberately opaque; everything actionable is here.
        f.debug_struct("BacklightDevice")
            .field("fs", &"(fs seam)")
            .field("root", &self.root)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("max", &self.max)
            .field("floor", &self.floor)
            .finish()
    }
}

/// Round `num / den` half-away-from-zero on exact integers (no float
/// rounding drift; the ramp must be byte-reproducible).
fn div_round_half_away(num: i128, den: i128) -> i128 {
    debug_assert!(den > 0);
    let q = num / den;
    let r = num % den;
    if 2 * r.abs() >= den {
        q + r.signum()
    } else {
        q
    }
}

impl BacklightDevice {
    /// Open `/sys/class/backlight/<name>`.
    ///
    /// # Errors
    /// `BacklightIo`/`NotFound` when the directory or `max_brightness`
    /// is missing or unparsable.
    pub fn open(fs: std::sync::Arc<dyn BacklightFs + Send + Sync>, name: &str) -> Result<Self> {
        let root = PathBuf::from("/sys/class/backlight").join(name);
        let max_raw = fs.read(&root.join("max_brightness"))?;
        let Some(max_raw) = max_raw else {
            return Err(DisplayError::BacklightIo {
                path: root.join("max_brightness"),
                errno: -1,
            });
        };
        let max: u64 = max_raw.parse().map_err(|_| DisplayError::BacklightIo {
            path: root.join("max_brightness"),
            errno: -1,
        })?;
        if max == 0 {
            return Err(DisplayError::BacklightIo {
                path: root.join("max_brightness"),
                errno: -1,
            });
        }
        let kind = BacklightKind::parse(fs.read(&root.join("type"))?.as_deref().unwrap_or(""));
        let floor = kind.brightness_floor().min(max);
        Ok(Self {
            fs,
            root,
            name: name.to_owned(),
            kind,
            max,
            floor,
        })
    }

    /// The device name (sysfs directory name).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The driver kind.
    #[must_use]
    pub fn kind(&self) -> BacklightKind {
        self.kind
    }

    /// Maximum brightness level.
    #[must_use]
    pub fn max(&self) -> u64 {
        self.max
    }

    /// The platform brightness floor.
    #[must_use]
    pub fn floor(&self) -> u64 {
        self.floor
    }

    /// Current brightness.
    ///
    /// # Errors
    /// `BacklightIo` when the file is missing/unparsable.
    pub fn brightness(&self) -> Result<u64> {
        let raw = self
            .fs
            .read(&self.root.join("brightness"))?
            .ok_or_else(|| DisplayError::BacklightIo {
                path: self.root.join("brightness"),
                errno: -1,
            })?;
        let value: u64 = raw.parse().map_err(|_| DisplayError::BacklightIo {
            path: self.root.join("brightness"),
            errno: -1,
        })?;
        Ok(value.min(self.max))
    }

    /// Write a brightness level, clamped to `[floor, max]`.
    ///
    /// # Errors
    /// `BacklightIo` on write failure.
    pub fn set_brightness(&self, level: u64) -> Result<u64> {
        let clamped = level.clamp(self.floor, self.max);
        self.fs
            .write(&self.root.join("brightness"), &clamped.to_string())?;
        Ok(clamped)
    }

    /// A deterministic integer ramp from the current level to `target`
    /// over `steps` writes (1 = direct set). Interpolation is
    /// round-half-away-from-zero at each step, so the ramp is monotone
    /// and reproducible; the final write is always the exact target.
    ///
    /// # Errors
    /// Propagates read/write failures; on failure the device stays at
    /// the last successfully written level.
    pub fn ramp_to(&self, target: u64, steps: u32) -> Result<Vec<u64>> {
        let target = target.clamp(self.floor, self.max);
        let start = self.brightness()?;
        if steps <= 1 || start == target {
            let applied = self.set_brightness(target)?;
            return Ok(vec![applied]);
        }
        let mut levels = Vec::new();
        for i in 1..=u64::from(steps) {
            // start + (target - start) * i / steps: only the delta term
            // divides (the start must not be re-averaged), symmetric
            // rounding on that term alone.
            let delta = i128::from(target) - i128::from(start);
            let step = div_round_half_away(delta * i128::from(i), i128::from(steps));
            let level = i128::from(start) + step;
            let level = u64::try_from(level).unwrap_or(target);
            let level = level.clamp(self.floor, self.max);
            let applied = self.set_brightness(level)?;
            levels.push(applied);
        }
        debug_assert_eq!(levels.last().copied().unwrap_or(start), target);
        Ok(levels)
    }
}

/// In-memory [`BacklightFs`] for tests: a sysfs tree of string files.
///
/// Clones share the underlying tree (the `Arc`), mirroring how real
/// sysfs state is shared between the device handle and the test that
/// inspects it.
#[derive(Clone, Debug, Default)]
pub struct MemFs {
    files: std::sync::Arc<std::sync::Mutex<BTreeMap<PathBuf, String>>>,
}

impl MemFs {
    /// An empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed one file.
    ///
    /// # Panics
    /// Only if another thread poisoned the interior lock — impossible
    /// for a freshly constructed test tree.
    #[must_use]
    pub fn with(self, path: &str, content: &str) -> Self {
        self.files
            .lock()
            .expect("memfs poisoned")
            .insert(PathBuf::from(path), content.to_owned());
        self
    }

    /// The current content of one file.
    ///
    /// # Panics
    /// Only if another thread poisoned the interior lock — the test
    /// owner would have to panic mid-write, which the harness never
    /// does concurrently.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<String> {
        self.files
            .lock()
            .expect("memfs poisoned")
            .get(&PathBuf::from(path))
            .cloned()
    }
}

impl BacklightFs for MemFs {
    fn read(&self, path: &Path) -> Result<Option<String>> {
        Ok(self
            .files
            .lock()
            .expect("memfs poisoned")
            .get(path)
            .map(|s| s.trim().to_owned()))
    }

    fn write(&self, path: &Path, value: &str) -> Result<()> {
        self.files
            .lock()
            .expect("memfs poisoned")
            .insert(path.to_owned(), value.to_owned());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn device(max: u64, brightness: u64, kind: &str) -> (MemFs, BacklightDevice) {
        let fs = MemFs::new()
            .with("/sys/class/backlight/gpu0/max_brightness", &max.to_string())
            .with(
                "/sys/class/backlight/gpu0/brightness",
                &brightness.to_string(),
            )
            .with("/sys/class/backlight/gpu0/type", kind);
        let dev = BacklightDevice::open(Arc::new(fs.clone()), "gpu0").unwrap();
        (fs, dev)
    }

    #[test]
    fn open_parses_kind_and_max() {
        let (_, dev) = device(255, 128, "raw");
        assert_eq!(dev.kind(), BacklightKind::Raw);
        assert_eq!(dev.max(), 255);
        assert_eq!(dev.floor(), 1);
        assert_eq!(dev.name(), "gpu0");
        let (_, fw) = device(100, 50, "firmware");
        assert_eq!(fw.floor(), 0);
        let (_, unk) = device(100, 50, "weird");
        assert_eq!(unk.kind(), BacklightKind::Unknown);
    }

    #[test]
    fn missing_device_rejected() {
        let err = BacklightDevice::open(Arc::new(MemFs::new()), "absent").unwrap_err();
        assert!(matches!(err, DisplayError::BacklightIo { .. }));
    }

    #[test]
    fn writes_clamp_to_floor_and_max() {
        let (fs, dev) = device(255, 128, "raw");
        assert_eq!(dev.set_brightness(0).unwrap(), 1); // raw floor
        assert_eq!(dev.set_brightness(999).unwrap(), 255);
        assert_eq!(
            fs.get("/sys/class/backlight/gpu0/brightness").unwrap(),
            "255"
        );
        assert_eq!(dev.brightness().unwrap(), 255);
    }

    #[test]
    fn ramp_is_monotone_and_lands_on_target() {
        let (fs, dev) = device(100, 10, "platform");
        // Uniform interpolation: 10 + 90*i/5 for i = 1..=5.
        let levels = dev.ramp_to(100, 5).unwrap();
        assert_eq!(levels, vec![28, 46, 64, 82, 100]);
        assert_eq!(
            fs.get("/sys/class/backlight/gpu0/brightness").unwrap(),
            "100"
        );
        // Downward ramp too (exact quarters).
        let levels = dev.ramp_to(0, 4).unwrap();
        assert_eq!(levels, vec![75, 50, 25, 0]);
        // Half-way deltas round away from zero: +2.5 rounds to +3,
        // -2.5 to -3, so 0 -> 5 in 2 steps is [3, 5] and 5 -> 0 is
        // [2, 0].
        let (_, dev) = device(5, 0, "firmware");
        assert_eq!(dev.ramp_to(5, 2).unwrap(), vec![3, 5]);
        assert_eq!(dev.ramp_to(0, 2).unwrap(), vec![2, 0]);
        // One step = direct.
        let levels = dev.ramp_to(4, 1).unwrap();
        assert_eq!(levels, vec![4]);
        // Already at target.
        let levels = dev.ramp_to(4, 8).unwrap();
        assert_eq!(levels, vec![4]);
    }

    #[test]
    fn kinds_parse() {
        assert_eq!(BacklightKind::parse("platform\n"), BacklightKind::Platform);
        assert_eq!(BacklightKind::parse("panel"), BacklightKind::Panel);
        assert_eq!(BacklightKind::parse("nope"), BacklightKind::Unknown);
    }
}
