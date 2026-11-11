//! Supervisory task service; all model work remains in the official SDK host.
use crate::{
    AgentBackend, AgentCommand, AgentConfig, AgentEvent,
    tasks::{self, Coordinator, State},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};
pub enum Command {
    Add {
        root: PathBuf,
        objective: String,
        dependencies: Vec<u64>,
        validation: Vec<String>,
    },
    RunReady,
    Decide {
        request: u64,
        allow: bool,
    },
    Integrate {
        id: u64,
        commit: String,
    },
    Retry(u64),
    CancelAll,
}
pub enum Event {
    Report(String),
    Log(String),
    Permission { request: u64, description: String },
    Stopped,
}
pub struct Host {
    tx: SyncSender<Command>,
    rx: Receiver<Event>,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}
impl Host {
    pub fn spawn(
        data: PathBuf,
        backend: impl FnOnce(PathBuf) -> Result<Box<dyn AgentBackend>, String> + Send + 'static,
    ) -> Result<Self, String> {
        let (tx, commands) = mpsc::sync_channel(16);
        let (events, rx) = mpsc::sync_channel(32);
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let (st, ct) = (stop.clone(), cancel.clone());
        let done = Arc::new(AtomicBool::new(false));
        let finished = done.clone();
        std::thread::Builder::new()
            .name("rivet-tasks".into())
            .spawn(move || {
                let result = backend(data.join("task-runtime")).and_then(|mut sdk| {
                    let result = serve(data, &mut *sdk, commands, &events, &st, &ct);
                    sdk.shutdown();
                    let deadline = std::time::Instant::now() + Duration::from_secs(10);
                    while std::time::Instant::now() < deadline {
                        if matches!(sdk.try_event(), Some(AgentEvent::Stopped)) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    result
                });
                if let Err(e) = result {
                    let _ = events.try_send(Event::Log(e));
                }
                let _ = events.try_send(Event::Stopped);
                finished.store(true, Ordering::SeqCst);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx,
            rx,
            stop,
            cancel,
            done,
        })
    }
    pub fn submit(&self, command: Command) -> Result<(), String> {
        if matches!(command, Command::CancelAll) {
            self.cancel.store(true, Ordering::Relaxed);
        }
        self.tx
            .try_send(command)
            .map_err(|_| "Task command queue full or disconnected".into())
    }
    pub fn try_event(&self) -> Option<Event> {
        self.rx.try_recv().ok()
    }
    pub fn is_finished(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn emit(tx: &SyncSender<Event>, event: Event, stop: &AtomicBool) -> Result<(), String> {
    let mut event = event;
    loop {
        match tx.try_send(event) {
            Ok(()) => return Ok(()),
            Err(mpsc::TrySendError::Disconnected(_)) => return Err("Task view disconnected".into()),
            Err(mpsc::TrySendError::Full(e)) => {
                if stop.load(Ordering::Relaxed) {
                    return Err("Task service stopping".into());
                }
                event = e;
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
fn serve(
    data: PathBuf,
    sdk: &mut dyn AgentBackend,
    commands: Receiver<Command>,
    events: &SyncSender<Event>,
    stop: &AtomicBool,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut coordinator = Coordinator::open(&data.join("tasks.journal"))?;
    let worktrees = data.join("worktrees");
    std::fs::create_dir_all(&worktrees).map_err(|e| e.to_string())?;
    emit(events, Event::Report(coordinator.report()), stop)?;
    let mut schedule = false;
    while !stop.load(Ordering::Relaxed) {
        match commands.try_recv() {
            Ok(command) => {
                let result = (|| -> Result<(), String> {
                    match command {
                        Command::Add {
                            root,
                            objective,
                            dependencies,
                            validation,
                        } => {
                            let root = root.canonicalize().map_err(|e| e.to_string())?;
                            coordinator.add(root, objective, dependencies, validation)?;
                        }
                        Command::RunReady => {
                            cancel.store(false, Ordering::Relaxed);
                            schedule = true;
                        }
                        Command::Retry(id) => coordinator.retry(id)?,
                        Command::Decide { request, allow } => {
                            sdk.submit(AgentCommand::Decide { request, allow })?
                        }
                        Command::CancelAll => {
                            schedule = false;
                            let ids: Vec<_> = coordinator
                                .tasks
                                .values()
                                .filter(|t| matches!(t.state, State::Running | State::Validating))
                                .map(|t| t.id)
                                .collect();
                            for id in ids {
                                let _ = sdk.submit(AgentCommand::Cancel(session_key(
                                    &coordinator.tasks[&id],
                                )));
                                coordinator.cancel(id)?;
                            }
                        }
                        Command::Integrate { id, commit } => {
                            let task = coordinator.tasks.get(&id).cloned().ok_or("Unknown task")?;
                            match tasks::integrate(&task, &commit, cancel) {
                                Ok(head) => coordinator.integrated(id, head)?,
                                Err(e) => {
                                    coordinator.fail(id,format!("Integration/combined validation failed; inspect main workspace without resetting it: {e}"))?;
                                }
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = result {
                    emit(events, Event::Log(e), stop)?;
                }
                emit(events, Event::Report(coordinator.report()), stop)?;
            }
            Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        for _ in 0..32 {
            let Some(event) = sdk.try_event() else {
                break;
            };
            // Reject late events/permissions from cancelled or superseded attempts.
            let key = match &event {
                AgentEvent::Ready { key, .. }
                | AgentEvent::Finished { key }
                | AgentEvent::Error { key, .. }
                | AgentEvent::Delta { key, .. }
                | AgentEvent::Activity { key, .. }
                | AgentEvent::Permission { key, .. } => Some(*key),
                _ => None,
            };
            if let Some(key) = key
                && !coordinator
                    .tasks
                    .get(&(key / 3))
                    .is_some_and(|task| session_key(task) == key && task.state == State::Running)
            {
                if let AgentEvent::Permission { request, .. } = event {
                    let _ = sdk.submit(AgentCommand::Decide {
                        request,
                        allow: false,
                    });
                }
                continue;
            }
            let changed = matches!(
                &event,
                AgentEvent::Ready { .. } | AgentEvent::Finished { .. } | AgentEvent::Error { .. }
            );
            match event {
                AgentEvent::Ready { key, id: sdk_id } => {
                    let wire_key = key;
                    let key = key / 3;
                    if let Some(task) = coordinator
                        .tasks
                        .get(&key)
                        .cloned()
                        .filter(|t| t.state == State::Running)
                    {
                        coordinator.session(key, sdk_id)?;
                        let prompt = format!(
                            "{}\n\nCompletion criteria: validation command {:?} must succeed. Work only in the assigned worktree. Report results for human review. Do not push, deploy, or integrate into the main workspace. Commit changes only after explicit user permission.",
                            task.objective, task.validation
                        );
                        if let Err(e) = sdk.submit(AgentCommand::Send {
                            key: wire_key,
                            prompt,
                        }) {
                            coordinator.fail(key, e)?;
                        }
                    }
                }
                AgentEvent::Finished { key } => {
                    let key = key / 3;
                    if let Some(task) = coordinator
                        .tasks
                        .get(&key)
                        .cloned()
                        .filter(|t| t.state == State::Running)
                    {
                        coordinator.agent_finished(key)?;
                        match tasks::validate(
                            &task,
                            task.worktree.as_ref().ok_or("Missing task worktree")?,
                            cancel,
                        ) {
                            Ok(log) => {
                                coordinator.validated(key)?;
                                emit(
                                    events,
                                    Event::Log(format!(
                                        "Task #{key} validated. Review changes in its worktree, commit reviewed changes there, then approve the exact commit SHA for integration.\n{log}"
                                    )),
                                    stop,
                                )?;
                            }
                            Err(e) => coordinator.fail(key, e)?,
                        }
                    }
                }
                AgentEvent::Error { key, message } => {
                    let key = key / 3;
                    if coordinator.tasks.contains_key(&key)
                        && !matches!(
                            coordinator.tasks[&key].state,
                            State::Cancelled | State::Completed
                        )
                    {
                        coordinator.fail(key, message)?;
                    }
                }
                AgentEvent::Permission {
                    key,
                    request,
                    description,
                } => {
                    emit(
                        events,
                        Event::Permission {
                            request,
                            description: format!("Task #{}\n{description}", key / 3),
                        },
                        stop,
                    )?;
                }
                AgentEvent::Delta { key, text } => {
                    emit(
                        events,
                        Event::Log(format!("Task #{}: {text}", key / 3)),
                        stop,
                    )?;
                }
                AgentEvent::Status(text) => {
                    emit(events, Event::Log(text), stop)?;
                }
                AgentEvent::Stopped => {
                    return Err(
                        "Task SDK host stopped; retained worktrees require inspection".into(),
                    );
                }
                _ => {}
            }
            if changed {
                emit(events, Event::Report(coordinator.report()), stop)?;
            }
        }
        if schedule && !cancel.load(Ordering::Relaxed) {
            let active = coordinator
                .tasks
                .values()
                .filter(|t| matches!(t.state, State::Running | State::Validating))
                .count();
            let ready: Vec<_> = coordinator
                .tasks
                .values()
                .filter(|t| t.state == State::Ready)
                .take(3usize.saturating_sub(active))
                .cloned()
                .collect();
            for task in ready {
                let destination =
                    worktrees.join(format!("task-{}-attempt-{}", task.id, 2 - task.retries));
                match tasks::prepare(&task.workspace, &destination, cancel) {
                    Ok(base) => {
                        coordinator.start(task.id, destination.clone(), base)?;
                        if let Err(e) = sdk.submit(AgentCommand::Open {
                            key: session_key(&task),
                            workspace: destination,
                            resume: None,
                            config: AgentConfig::default(),
                        }) {
                            coordinator.fail(task.id, e)?;
                        }
                    }
                    Err(e) => coordinator.fail(task.id, e)?,
                }
                emit(events, Event::Report(coordinator.report()), stop)?;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn session_key(task: &tasks::Task) -> u64 {
    task.id * 3 + u64::from(2 - task.retries)
}
