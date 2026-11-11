//! Synchronous Stage 1 probe boundary. Call on a worker thread in future UI code.
//! No SDK or Tokio types escape this crate.
use github_copilot_sdk::handler::DenyAllHandler;
use github_copilot_sdk::{
    Client, ClientOptions, MessageOptions, ResumeSessionConfig, SessionConfig,
};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct ProbeOptions {
    pub workspace: PathBuf,
    pub prompt: Option<String>,
    pub resume: Option<String>,
    pub cancel_after: Option<Duration>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Authenticated(bool),
    Session(String),
    Text(String),
    Activity(String),
    Cancelled,
    Completed,
    Timing {
        operation: &'static str,
        micros: u128,
    },
}

/// Runs SDK work on a private current-thread Tokio runtime, never a UI thread.
/// The callback must return promptly; Stage 1's CLI writes directly to stdout.
/// The deadline includes startup/authentication and the turn, but not cleanup.
pub fn probe(options: ProbeOptions, mut emit: impl FnMut(Event)) -> Result<(), String> {
    validate(&options)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    runtime.block_on(async {
        let deadline = tokio::time::Instant::now() + options.timeout;
        let started = Instant::now();
        let mut config = ClientOptions::default();
        config.working_directory = options.workspace.clone();
        let client = tokio::time::timeout_at(deadline, Client::start(config))
            .await
            .map_err(|_| "SDK startup timed out".to_string())?
            .map_err(|e| format!("SDK startup: {e}"))?;
        emit(Event::Timing {
            operation: "sdk_startup",
            micros: started.elapsed().as_micros(),
        });
        let result = tokio::time::timeout_at(deadline, run(&client, &options, &mut emit))
            .await
            .map_err(|_| "probe timed out".to_string())
            .and_then(|r| r);
        // Always attempt cleanup, including authentication, stream, and timeout failures.
        if result.is_err() {
            client.force_stop();
        }
        let cleanup = tokio::time::timeout(Duration::from_secs(25), client.stop()).await;
        match cleanup {
            Ok(Ok(())) => result,
            _ => {
                client.force_stop();
                result.and(Err("SDK shutdown failed or timed out".into()))
            }
        }
    })
}

fn validate(options: &ProbeOptions) -> Result<(), String> {
    if !options.workspace.is_dir() {
        return Err("workspace must be an existing directory".into());
    }
    if options.timeout.is_zero() {
        return Err("timeout must be positive".into());
    }
    if options.prompt.as_ref().is_some_and(|p| p.trim().is_empty()) {
        return Err("prompt must not be empty".into());
    }
    if options.resume.as_ref().is_some_and(|s| s.trim().is_empty()) {
        return Err("session ID must not be empty".into());
    }
    if options.cancel_after.is_some() && options.prompt.is_none() {
        return Err("cancellation requires a prompt".into());
    }
    Ok(())
}

async fn run(
    client: &Client,
    options: &ProbeOptions,
    emit: &mut impl FnMut(Event),
) -> Result<(), String> {
    let auth = client
        .get_auth_status()
        .await
        .map_err(|e| format!("authentication check: {e}"))?;
    emit(Event::Authenticated(auth.is_authenticated));
    if !auth.is_authenticated {
        return Err(
            "Copilot is not authenticated. Sign in with Copilot CLI before running the probe."
                .into(),
        );
    }
    if options.prompt.is_none() && options.resume.is_none() {
        return Ok(());
    }
    let session = if let Some(id) = &options.resume {
        let mut config = ResumeSessionConfig::new(id.clone().into());
        config.streaming = Some(true);
        client
            .resume_session(config.with_permission_handler(Arc::new(DenyAllHandler)))
            .await
    } else {
        let mut config = SessionConfig::default();
        config.streaming = Some(true);
        config.name = Some("Rivet SDK feasibility".into());
        client
            .create_session(config.with_permission_handler(Arc::new(DenyAllHandler)))
            .await
    }
    .map_err(|e| format!("session create/resume: {e}"))?;
    emit(Event::Session(session.id().to_string()));
    let result = async {
        if let Some(prompt) = &options.prompt {
            let mut events = session.subscribe();
            let started = Instant::now();
            // Let the SDK correlate completion with this request, including on resume.
            // An old session.idle event must not complete a new turn.
            let turn = session.send_and_wait(
                MessageOptions::new(prompt.clone()).with_wait_timeout(options.timeout)
            );
            tokio::pin!(turn);
            let cancel = tokio::time::sleep(options.cancel_after.unwrap_or(options.timeout));
            tokio::pin!(cancel);
            let mut first_text = true;
            loop {
                tokio::select! {
                    biased;
                    _ = &mut cancel, if options.cancel_after.is_some() => {
                        session.abort().await.map_err(|e| format!("cancel: {e}"))?;
                        emit(Event::Cancelled);
                        break;
                    }
                    event = events.recv() => {
                        let event = event.map_err(|e| format!("event stream (possible lag or disconnect): {e}"))?;
                        match event.event_type.as_str() {
                            "assistant.message_delta" => {
                                if let Some(text) = event.data.get("delta").and_then(|v| v.as_str()) {
                                    if first_text {
                                        emit(Event::Timing { operation: "send_to_first_delta", micros: started.elapsed().as_micros() });
                                        first_text = false;
                                    }
                                    emit(Event::Text(text.into()));
                                }
                            }
                            "session.idle" => {} // SDK completion future is authoritative.
                            "session.error" => {
                                if event.data.get("errorType").and_then(|v| v.as_str()) == Some("authorization") {
                                    return Err("Copilot rejected model authorization. Auth status alone does not prove model access; refresh Copilot login and check entitlement.".into());
                                }
                                return Err("Copilot reported a session error; inspect its local logs".into());
                            }
                            kind => emit(Event::Activity(kind.into())),
                        }
                    }
                    result = &mut turn => {
                        result.map_err(|e| format!("agent turn: {e}"))?;
                        emit(Event::Completed);
                        break;
                    }
                }
            }
        }
        Ok(())
    }.await;
    let disconnect = session
        .disconnect()
        .await
        .map_err(|e| format!("disconnect: {e}"));
    result.and(disconnect)
}

mod host;
pub use host::AgentHost;

#[cfg(test)]
mod tests {
    use super::*;
    fn options() -> ProbeOptions {
        ProbeOptions {
            workspace: std::env::current_dir().unwrap(),
            prompt: None,
            resume: None,
            cancel_after: None,
            timeout: Duration::from_secs(1),
        }
    }
    #[test]
    fn rejects_invalid_requests_before_starting_sdk() {
        let mut o = options();
        o.timeout = Duration::ZERO;
        assert_eq!(validate(&o).unwrap_err(), "timeout must be positive");
        let mut o = options();
        o.cancel_after = Some(Duration::ZERO);
        assert!(validate(&o).unwrap_err().contains("requires a prompt"));
        let mut o = options();
        o.prompt = Some("  ".into());
        assert!(validate(&o).unwrap_err().contains("empty"));
    }
    #[test]
    fn permission_policy_rejects_unknown_requests() {
        use github_copilot_sdk::{
            PermissionRequestData,
            handler::{PermissionHandler, PermissionResult},
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let data = PermissionRequestData {
            kind: None,
            tool_call_id: None,
            managed_approval_required: Some(true),
            managed_settings_enabled: true,
            extra: Default::default(),
        };
        let result = runtime.block_on(DenyAllHandler.handle("test".into(), "request".into(), data));
        assert!(matches!(
            result,
            PermissionResult::Decision {
                decision: github_copilot_sdk::PermissionDecision::Reject(_),
                ..
            }
        ));
    }
}
