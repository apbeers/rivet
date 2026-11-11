//! Long-lived SDK sessions, isolated from native UI types and event loops.
use github_copilot_sdk::{
    Client, ClientOptions, MessageOptions, PermissionRequestData, RequestId, ResumeSessionConfig,
    SessionConfig, SessionId, SystemMessageConfig,
    handler::{PermissionHandler, PermissionResult},
    session::Session,
};
use rivet_core::{AgentCommand, AgentEvent, journal::Journal};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    time::Duration,
};
use tokio::sync::{mpsc as async_channel, oneshot};

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<bool>>>>;
struct Permissions {
    key: u64,
    next: Arc<AtomicU64>,
    pending: Pending,
    events: async_channel::Sender<AgentEvent>,
}
#[async_trait::async_trait]
impl PermissionHandler for Permissions {
    async fn handle(
        &self,
        _sid: SessionId,
        _rid: RequestId,
        data: PermissionRequestData,
    ) -> PermissionResult {
        let request = self.next.fetch_add(1, Ordering::Relaxed);
        let description: String = format!("{:?}: {}", data.kind, data.extra)
            .chars()
            .take(8192)
            .collect();
        let (tx, rx) = oneshot::channel();
        {
            let Ok(mut pending) = self.pending.lock() else {
                return PermissionResult::reject(None);
            };
            if pending.len() >= 32 {
                return PermissionResult::reject(None);
            }
            pending.insert(request, tx);
        }
        let sent = tokio::time::timeout(
            Duration::from_secs(5),
            self.events.send(AgentEvent::Permission {
                key: self.key,
                request,
                description,
            }),
        )
        .await;
        let approved = if matches!(sent, Ok(Ok(()))) {
            matches!(
                tokio::time::timeout(Duration::from_secs(120), rx).await,
                Ok(Ok(true))
            )
        } else {
            false
        };
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&request);
        }
        if approved {
            PermissionResult::approve_once()
        } else {
            PermissionResult::reject(None)
        }
    }
}

pub struct AgentHost {
    commands: SyncSender<AgentCommand>,
    events: async_channel::Receiver<AgentEvent>,
    stop: Arc<AtomicBool>,
    stopped: bool,
}
impl AgentHost {
    pub fn spawn(data_dir: PathBuf) -> Result<Self, String> {
        let (commands, rx) = mpsc::sync_channel(32);
        let (tx, events) = async_channel::channel(256);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        std::thread::Builder::new()
            .name("rivet-copilot".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(r) => r,
                    Err(e) => {
                        let _ =
                            tx.blocking_send(AgentEvent::Status(format!("Runtime failed: {e}")));
                        return;
                    }
                };
                runtime.block_on(serve(rx, tx, worker_stop, data_dir));
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            commands,
            events,
            stop,
            stopped: false,
        })
    }
    pub fn submit(&self, command: AgentCommand) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|_| "Agent command queue is full or disconnected".into())
    }
    pub fn try_event(&mut self) -> Option<AgentEvent> {
        match self.events.try_recv() {
            Ok(event) => {
                if matches!(event, AgentEvent::Stopped) {
                    self.stopped = true;
                }
                Some(event)
            }
            Err(async_channel::error::TryRecvError::Disconnected) if !self.stopped => {
                self.stopped = true;
                Some(AgentEvent::Stopped)
            }
            Err(_) => None,
        }
    }
}
impl Drop for AgentHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
async fn error(events: &async_channel::Sender<AgentEvent>, key: u64, message: impl ToString) {
    let _ = events
        .send(AgentEvent::Error {
            key,
            message: message.to_string(),
        })
        .await;
}
async fn serve(
    rx: mpsc::Receiver<AgentCommand>,
    events: async_channel::Sender<AgentEvent>,
    stop: Arc<AtomicBool>,
    dir: PathBuf,
) {
    let (mut journal, records) = match Journal::open(&dir.join("sessions.rvj")) {
        Ok(j) => j,
        Err(e) => {
            let _ = events
                .send(AgentEvent::Status(format!(
                    "Session store unavailable: {e}"
                )))
                .await;
            return;
        }
    };
    let mut saved = std::collections::HashSet::new();
    for (kind, bytes) in records
        .into_iter()
        .rev()
        .filter(|(kind, _)| *kind == 1)
        .take(64)
    {
        if kind == 1
            && let Ok(id) = String::from_utf8(bytes)
            && saved.insert(id.clone())
        {
            let _ = events.send(AgentEvent::SavedSession { id }).await;
        }
    }
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let next = Arc::new(AtomicU64::new(1));
    let mut client: Option<Client> = None;
    let mut sessions: BTreeMap<u64, Arc<Session>> = BTreeMap::new();
    let mut streams = Vec::new();
    let mut turns: HashMap<u64, tokio::task::JoinHandle<()>> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        let command = match rx.try_recv() {
            Ok(c) => c,
            Err(mpsc::TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            }
            Err(_) => break,
        };
        if let AgentCommand::Decide { request, allow } = &command {
            // Durability precedes authorizing the sensitive action. Failure denies.
            let durable = journal
                .append(
                    2,
                    format!("{request}:{}", if *allow { "allow" } else { "deny" }).as_bytes(),
                )
                .is_ok();
            if !durable {
                let _ = events
                    .send(AgentEvent::Status(
                        "Permission decision could not be saved; request denied".into(),
                    ))
                    .await;
            }
            if let Ok(mut requests) = pending.lock()
                && let Some(reply) = requests.remove(request)
            {
                let _ = reply.send(*allow && durable);
            }
            continue;
        }
        if client.is_none() && matches!(command, AgentCommand::Open { .. } | AgentCommand::Models) {
            let _ = events
                .send(AgentEvent::Status("Connecting to Copilot…".into()))
                .await;
            match tokio::time::timeout(
                Duration::from_secs(30),
                Client::start(ClientOptions::default()),
            )
            .await
            {
                Ok(Ok(c)) => {
                    client = Some(c);
                }
                _ => {
                    let _ = events
                        .send(AgentEvent::Status(
                            "Copilot startup failed or timed out; verify runtime and login".into(),
                        ))
                        .await;
                    if let AgentCommand::Open { key, .. } = &command {
                        error(&events, *key, "Copilot startup failed or timed out").await;
                    }
                    continue;
                }
            }
        }
        match command {
            AgentCommand::Open {
                key,
                workspace,
                resume,
                config,
            } => {
                if let Some(s) = sessions.get(&key) {
                    let _ = events
                        .send(AgentEvent::Ready {
                            key,
                            id: s.id().to_string(),
                        })
                        .await;
                    continue;
                }
                if sessions.len() >= 20 {
                    error(&events, key, "Session budget reached (20)").await;
                    continue;
                }
                let Some(c) = client.as_ref() else {
                    continue;
                };
                if !workspace.is_dir() {
                    error(&events, key, "Workspace does not exist").await;
                    continue;
                }
                let permissions = Arc::new(Permissions {
                    key,
                    next: next.clone(),
                    pending: pending.clone(),
                    events: events.clone(),
                });
                let create = async {
                    if let Some(id) = resume {
                        let mut cfg = ResumeSessionConfig::new(id.into());
                        cfg.streaming = Some(true);
                        cfg.working_directory = Some(workspace);
                        if !config.model.is_empty() {
                            cfg.model = Some(config.model);
                        }
                        if !config.instructions.is_empty() {
                            cfg.system_message =
                                Some(SystemMessageConfig::new().with_content(config.instructions));
                        }
                        c.resume_session(cfg.with_permission_handler(permissions))
                            .await
                    } else {
                        let mut cfg = SessionConfig::default();
                        cfg.streaming = Some(true);
                        cfg.working_directory = Some(workspace);
                        if !config.model.is_empty() {
                            cfg.model = Some(config.model);
                        }
                        if !config.instructions.is_empty() {
                            cfg.system_message =
                                Some(SystemMessageConfig::new().with_content(config.instructions));
                        }
                        c.create_session(cfg.with_permission_handler(permissions))
                            .await
                    }
                };
                match tokio::time::timeout(Duration::from_secs(30), create).await {
                    Ok(Ok(session)) => {
                        let session = Arc::new(session);
                        let id = session.id().to_string();
                        if let Err(e) = journal.append(1, id.as_bytes()) {
                            error(
                                &events,
                                key,
                                format!("Session reference could not be saved: {e}"),
                            )
                            .await;
                            let _ = session.disconnect().await;
                            continue;
                        }
                        let mut subscription = session.subscribe();
                        let output = events.clone();
                        streams.push(tokio::spawn(async move {
                            loop {match subscription.recv().await {
                                Ok(e)=>{
                                    let event=match e.event_type.as_str(){
                                        "assistant.message_delta"=>AgentEvent::Delta{key,text:e.data.get("delta").and_then(|v|v.as_str()).unwrap_or("").into()},
                                        "session.error"=>AgentEvent::Error{key,message:"Copilot session failed; verify model access and SDK logs".into()},
                                        "tool.execution_start"|"tool.execution_complete"|"assistant.turn_start"=>AgentEvent::Activity{key,kind:e.event_type},
                                        _=>continue,
                                    }; if output.send(event).await.is_err(){break;}
                                },Err(e)=>{error(&output,key,format!("Stream lost; reconnect session to resynchronize: {e}")).await;break;}
                            }}
                        }));
                        if let Ok(Ok(history)) =
                            tokio::time::timeout(Duration::from_secs(10), session.get_events())
                                .await
                        {
                            for event in history
                                .into_iter()
                                .rev()
                                .take(512)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                            {
                                if matches!(
                                    event.event_type.as_str(),
                                    "user.message" | "assistant.message"
                                ) && let Some(text) =
                                    event.data.get("content").and_then(|v| v.as_str())
                                {
                                    let _ = events
                                        .send(AgentEvent::Delta {
                                            key,
                                            text: format!("\n{}: {text}\n", event.event_type),
                                        })
                                        .await;
                                }
                            }
                        }
                        sessions.insert(key, session);
                        let _ = events
                            .send(AgentEvent::Ready {
                                key,
                                id: id.clone(),
                            })
                            .await;
                        if saved.insert(id.clone()) {
                            let _ = events.send(AgentEvent::SavedSession { id }).await;
                        }
                    }
                    Ok(Err(e)) => error(&events, key, e).await,
                    Err(_) => error(&events, key, "Session creation timed out").await,
                }
            }
            AgentCommand::Send { key, prompt } => {
                if prompt.trim().is_empty() || prompt.len() > 262144 {
                    error(&events, key, "Prompt empty or exceeds 256 KiB").await;
                    continue;
                }
                if turns.get(&key).is_some_and(|t| !t.is_finished()) {
                    error(&events, key, "A turn is already running").await;
                    continue;
                }
                let Some(session) = sessions.get(&key).cloned() else {
                    error(&events, key, "Connect the session first").await;
                    continue;
                };
                let output = events.clone();
                turns.insert(
                    key,
                    tokio::spawn(async move {
                        match session
                            .send_and_wait(
                                MessageOptions::new(prompt)
                                    .with_wait_timeout(Duration::from_secs(600)),
                            )
                            .await
                        {
                            Ok(_) => {
                                let _ = output.send(AgentEvent::Finished { key }).await;
                            }
                            Err(e) => error(&output, key, e).await,
                        }
                    }),
                );
            }
            AgentCommand::Cancel(key) => {
                if let Some(s) = sessions.get(&key) {
                    match tokio::time::timeout(Duration::from_secs(5), s.abort()).await {
                        Ok(Ok(())) => {}
                        _ => error(&events, key, "Cancellation failed or timed out").await,
                    }
                }
            }
            AgentCommand::Models => {
                if let Some(c) = client.as_ref() {
                    match tokio::time::timeout(Duration::from_secs(15), c.list_models()).await {
                        Ok(Ok(models)) => {
                            let _ = events
                                .send(AgentEvent::Models(
                                    models.into_iter().map(|m| m.id).collect(),
                                ))
                                .await;
                        }
                        _ => {
                            let _ = events
                                .send(AgentEvent::Status("Model list unavailable".into()))
                                .await;
                        }
                    }
                }
            }
            AgentCommand::Decide { .. } => unreachable!(),
        }
    }
    if let Ok(mut requests) = pending.lock() {
        requests.clear();
    }
    for (_, turn) in turns {
        turn.abort();
    }
    for stream in streams {
        stream.abort();
    }
    if let Some(c) = client
        && !matches!(
            tokio::time::timeout(Duration::from_secs(5), c.stop()).await,
            Ok(Ok(()))
        )
    {
        c.force_stop();
    }
    let _ = events.send(AgentEvent::Stopped).await;
}

impl rivet_core::AgentBackend for AgentHost {
    fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
    fn submit(&self, c: AgentCommand) -> Result<(), String> {
        AgentHost::submit(self, c)
    }
    fn try_event(&mut self) -> Option<AgentEvent> {
        AgentHost::try_event(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> PermissionRequestData {
        PermissionRequestData {
            kind: None,
            tool_call_id: None,
            managed_approval_required: Some(true),
            managed_settings_enabled: true,
            extra: Default::default(),
        }
    }
    #[test]
    fn permission_handler_requires_a_specific_reply_and_denies_disconnection() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            for allow in [false, true] {
                let (events, mut rx) = async_channel::channel(4);
                let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
                let handler = Permissions {
                    key: 7,
                    next: Arc::new(AtomicU64::new(1)),
                    pending: pending.clone(),
                    events,
                };
                let task = tokio::spawn(async move {
                    handler
                        .handle("session".into(), "request".into(), request())
                        .await
                });
                let AgentEvent::Permission { key, request, .. } = rx.recv().await.unwrap() else {
                    panic!("permission event required")
                };
                assert_eq!(key, 7);
                pending
                    .lock()
                    .unwrap()
                    .remove(&request)
                    .unwrap()
                    .send(allow)
                    .unwrap();
                let result = task.await.unwrap();
                assert_eq!(
                    matches!(
                        result,
                        PermissionResult::Decision {
                            decision: github_copilot_sdk::PermissionDecision::ApproveOnce(_),
                            ..
                        }
                    ),
                    allow
                );
                assert!(pending.lock().unwrap().is_empty());
            }
            let (events, rx) = async_channel::channel(1);
            drop(rx);
            let handler = Permissions {
                key: 1,
                next: Arc::new(AtomicU64::new(1)),
                pending: Arc::new(Mutex::new(HashMap::new())),
                events,
            };
            assert!(matches!(
                handler.handle("s".into(), "r".into(), request()).await,
                PermissionResult::Decision {
                    decision: github_copilot_sdk::PermissionDecision::Reject(_),
                    ..
                }
            ));
        });
    }
}
