//! Native AppKit frontend. No platform types cross this crate's public boundary.
#[cfg(target_os = "macos")]
mod appkit;
#[cfg(target_os = "macos")]
mod live;
#[cfg(target_os = "macos")]
pub fn run(smoke: bool, backend: Option<Box<dyn rivet_core::AgentBackend>>) -> Result<(), String> {
    if smoke {
        appkit::run(true)
    } else {
        live::run(backend.ok_or("Live agent backend required")?, false)
    }
}

#[cfg(target_os = "macos")]
pub fn run_live_smoke() -> Result<(), String> {
    live::run(live::smoke_backend(), true)
}

#[cfg(target_os = "macos")]
mod browser;
#[cfg(target_os = "macos")]
mod documents;
