//! The bridge session tests' shared bring-up: the in-process
//! compositor over its abstract socket (the same shape the
//! lion-compositor suite's testbench runs, minimal for the bridge's
//! needs).

#![allow(dead_code)]

use std::sync::Arc;

use lion_compositor::server::{Compositor, CompositorConfig};
use lion_compositor::Shared;

/// A running in-process compositor.
pub struct Testbench {
    /// The shared world (scanout introspection).
    pub shared: Arc<Shared>,
    /// The listening abstract address.
    pub addr: ldp_transport::UnixAddr,
    _accept: std::thread::JoinHandle<()>,
}

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Testbench {
    /// Bring up the compositor and its accept loop.
    pub fn start(tag: &str) -> Testbench {
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let config = CompositorConfig {
            socket: format!("lion-bridge-it-{tag}-{n}-{}", std::process::id()),
            ..CompositorConfig::default()
        };
        let compositor = Compositor::headless(config).expect("headless bring-up");
        let addr = compositor.addr.clone();
        let shared = Arc::clone(&compositor.shared);
        let accept = std::thread::Builder::new()
            .name(format!("accept-{tag}"))
            .spawn(move || {
                let mut compositor = compositor;
                // Runs until the test process ends.
                let _ = compositor.serve_blocking();
            })
            .expect("accept thread");
        Testbench {
            shared,
            addr,
            _accept: accept,
        }
    }

    /// Bring up the compositor with an explicit config (the rootless
    /// suite serves the positioning shell: the dock on, the cascade
    /// placing the X windows' toplevels).
    pub fn start_with(tag: &str, config: CompositorConfig) -> Testbench {
        let compositor = Compositor::headless(config).expect("headless bring-up");
        let addr = compositor.addr.clone();
        let shared = Arc::clone(&compositor.shared);
        let accept = std::thread::Builder::new()
            .name(format!("accept-{tag}"))
            .spawn(move || {
                let mut compositor = compositor;
                let _ = compositor.serve_blocking();
            })
            .expect("accept thread");
        Testbench {
            shared,
            addr,
            _accept: accept,
        }
    }

    /// The compositor's current scanout (premultiplied ARGB words).
    pub fn scanout(&self) -> Vec<u32> {
        let world = self.shared.world.lock().expect("world lock");
        world.scanout_words().expect("the compositor is lit")
    }

    /// Write-side world access (the input rig).
    pub fn world_mut<R>(&self, f: impl FnOnce(&mut lion_compositor::World) -> R) -> R {
        let mut world = self.shared.world.lock().expect("world lock");
        f(&mut world)
    }

    /// Frames rendered so far.
    pub fn frames(&self) -> u64 {
        let world = self.shared.world.lock().expect("world lock");
        world.frames
    }
}
