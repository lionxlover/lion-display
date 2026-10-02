//! Raw frame dumps: the headless snapshot mode's on-disk form.
//!
//! `--dump DIR` writes one binary PPM (P6) per rendered frame — the
//! scanout buffer's premultiplied ARGB words un-premultiplied to plain
//! RGB (XRGB scanout ignores the alpha byte; a dumped frame therefore
//! shows exactly what a display would). Files are named
//! `frame_<NNNN>.ppm` in frame order; failures are silent (dumping is
//! diagnostics, never a pipeline dependency).

#![forbid(unsafe_code)]

use std::io::Write;
use std::path::Path;

use ldp_renderer::OutputDesc;

/// Write one frame as `frame_<NNNN>.ppm` under `dir`.
pub fn write_frame(dir: &Path, index: u64, desc: &OutputDesc, words: &[u32]) {
    let name = dir.join(format!("frame_{index:04}.ppm"));
    let file = std::fs::File::create(&name);
    let Ok(mut file) = file else { return };
    let header = format!("P6\n{} {}\n255\n", desc.width, desc.height);
    if file.write_all(header.as_bytes()).is_err() {
        return;
    }
    let mut rgb = Vec::with_capacity(words.len() * 3);
    for word in words {
        // Premultiplied ARGB word → straight RGB at full alpha: the
        // scanout pipeline composites over opaque black.
        let a = (word >> 24) & 0xFF;
        let r = (word >> 16) & 0xFF;
        let g = (word >> 8) & 0xFF;
        let b = word & 0xFF;
        let (r, g, b) = if a == 0 {
            (0, 0, 0)
        } else if a == 255 {
            (r, g, b)
        } else {
            // Un-premultiply with rounding; the alpha byte of an XRGB
            // scanout is padding (0xFF on the cleared path), so this
            // arm only serves partially-composited dumps.
            (
                ((r * 255 + a / 2) / a).min(255),
                ((g * 255 + a / 2) / a).min(255),
                ((b * 255 + a / 2) / a).min(255),
            )
        };
        rgb.extend_from_slice(&[r as u8, g as u8, b as u8]);
    }
    let _ = file.write_all(&rgb);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::buffer::FourCC;
    use ldp_core::color::ColorDescription;

    #[test]
    fn writes_a_readable_ppm() {
        let dir = std::env::temp_dir().join(format!("lion-dump-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let desc = OutputDesc::new(2, 1, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
        // Opaque red, opaque blue.
        write_frame(&dir, 7, &desc, &[0xFF_FF_00_00, 0xFF_00_00_FF]);
        let bytes = std::fs::read(dir.join("frame_0007.ppm")).unwrap();
        assert_eq!(&bytes[..11], b"P6\n2 1\n255\n");
        assert_eq!(&bytes[11..], &[0xFF, 0x00, 0x00, 0x00, 0x00, 0xFF]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
