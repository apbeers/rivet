//! A real SDK integration probe; never run by credential-free CI.
use rivet_copilot::AgentHost;
use rivet_core::{AgentCommand, AgentConfig, AgentEvent};
fn main() -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("rivet-live-probe-{}", std::process::id()));
    let mut host = AgentHost::spawn(dir)?;
    host.submit(AgentCommand::Open {
        key: 1,
        workspace: std::env::current_dir().map_err(|e| e.to_string())?,
        resume: None,
        config: AgentConfig::default(),
    })?;
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(90) {
        match host.try_event() {
            Some(AgentEvent::Ready { .. }) => host.submit(AgentCommand::Send {
                key: 1,
                prompt: "Reply with exactly Rivet. Do not use tools.".into(),
            })?,
            Some(AgentEvent::Delta { text, .. }) => print!("{text}"),
            Some(AgentEvent::Permission { request, .. }) => host.submit(AgentCommand::Decide {
                request,
                allow: false,
            })?,
            Some(AgentEvent::Finished { .. }) => {
                println!("\nReal host turn finished");
                return Ok(());
            }
            Some(AgentEvent::Error { message, .. }) => return Err(message),
            Some(AgentEvent::Status(s)) => eprintln!("{s}"),
            _ => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Err("Real host probe timed out".into())
}
