//! sling — throw a window at a task.
//!
//! The flow is written against traits (`WindowManager`, `Prompt`) so it can be
//! driven without AeroSpace or a screen. See `docs/SPEC.md` for what this is
//! for and `docs/FINDINGS.md` for the constraints the design works around.

pub mod aerospace;
pub mod app;
pub mod config;
pub mod dialog;
pub mod herdr;
pub mod panel;
pub mod picker;
pub mod store;
pub mod theme;
pub mod watch;
