//! Native Windows frontend. All SDK and async runtime types stay outside this crate.
#[cfg(windows)]
mod browser;
#[cfg(windows)]
mod sys;
#[cfg(windows)]
mod terminal;
#[cfg(windows)]
mod ui;
#[cfg(windows)]
pub use ui::{attach_parent_console, run, run_smoke};

#[cfg(windows)]
mod save;

#[cfg(windows)]
pub use browser::runtime_version as browser_runtime_version;
