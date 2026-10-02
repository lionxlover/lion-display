//! The seven tool entry points.
//!
//! Each module exports `pub fn run(Args, &mut dyn Write) ->
//! Result<i32, ToolError>`-shaped logic that the thin binaries in
//! `src/bin/` call after argv parsing; the integration suites call
//! the same functions (and spawn the same binaries) against a live
//! Phase 10 compositor.

pub mod audit;
pub mod debug;
pub mod grab;
pub mod info;
pub mod input_debug;
pub mod profiler;
pub mod validate;
