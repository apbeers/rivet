//! Native AppKit frontend. No platform types cross this crate's public boundary.
#[cfg(target_os = "macos")]
mod appkit;
#[cfg(target_os = "macos")]
pub use appkit::run;
