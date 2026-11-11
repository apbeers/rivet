//! Durable task graph. Only the coordinator changes state; model output is never completion proof.
use crate::{
    journal::Journal,
    json::{self, Value},
    process::{self, Spec},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Queued,
    Ready,
    Running,
    AwaitingApproval,
    Validating,
    Completed,
    Failed,
    Cancelled,
}
impl State {
    fn name(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Ready => "Ready",
            Self::Running => "Running",
            Self::AwaitingApproval => "AwaitingApproval",
            Self::Validating => "Validating",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
    fn parse(s: &str) -> Result<Self, String> {
        Ok(match s {
            "Queued" => Self::Queued,
            "Ready" => Self::Ready,
            "Running" => Self::Running,
            "AwaitingApproval" => Self::AwaitingApproval,
            "Validating" => Self::Validating,
            "Completed" => Self::Completed,
            "Failed" => Self::Failed,
            "Cancelled" => Self::Cancelled,
            _ => return Err("Unknown task state".into()),
        })
    }
}
#[derive(Clone, Debug)]
pub struct Task {
    pub id: u64,
    pub objective: String,
    pub dependencies: Vec<u64>,
    pub state: State,
    pub workspace: PathBuf,
    pub worktree: Option<PathBuf>,
    pub base: Option<String>,
    pub session: Option<String>,
    pub validation: Vec<String>,
    pub error: Option<String>,
    pub artifact: Option<String>,
    pub retries: u8,
}
fn string(v: &Value, key: &str) -> Result<String, String> {
    v.get(key)
        .and_then(Value::str)
        .map(String::from)
        .ok_or_else(|| format!("Missing {key}"))
}
impl Task {
    fn value(&self) -> Value {
        Value::object([
            ("id", Value::Number(self.id as f64)),
            ("objective", self.objective.clone().into()),
            (
                "dependencies",
                Value::Array(
                    self.dependencies
                        .iter()
                        .map(|&n| Value::Number(n as f64))
                        .collect(),
                ),
            ),
            ("state", self.state.name().into()),
            (
                "workspace",
                self.workspace.to_string_lossy().to_string().into(),
            ),
            (
                "worktree",
                self.worktree
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string().into())
                    .unwrap_or(Value::Null),
            ),
            (
                "base",
                self.base.clone().map(Value::from).unwrap_or(Value::Null),
            ),
            (
                "session",
                self.session.clone().map(Value::from).unwrap_or(Value::Null),
            ),
            (
                "validation",
                Value::Array(self.validation.iter().cloned().map(Value::from).collect()),
            ),
            (
                "error",
                self.error.clone().map(Value::from).unwrap_or(Value::Null),
            ),
            (
                "artifact",
                self.artifact
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            ("retries", Value::Number(self.retries as f64)),
        ])
    }
    fn parse(v: &Value) -> Result<Self, String> {
        let array = |key| {
            v.get(key)
                .and_then(Value::array)
                .ok_or_else(|| format!("Missing {key}"))
        };
        let id = |n: &Value| {
            n.integer()
                .filter(|&n| n > 0 && n < 1_000_000)
                .ok_or("Invalid task ID")
        };
        Ok(Self {
            id: id(v.get("id").ok_or("Missing task ID")?)?,
            objective: string(v, "objective")?,
            dependencies: array("dependencies")?
                .iter()
                .map(id)
                .collect::<Result<_, _>>()?,
            state: State::parse(&string(v, "state")?)?,
            workspace: PathBuf::from(string(v, "workspace")?),
            worktree: v.get("worktree").and_then(Value::str).map(PathBuf::from),
            base: v.get("base").and_then(Value::str).map(String::from),
            session: v.get("session").and_then(Value::str).map(String::from),
            validation: array("validation")?
                .iter()
                .map(|v| {
                    v.str()
                        .map(String::from)
                        .ok_or("Invalid validation argument")
                })
                .collect::<Result<_, _>>()?,
            error: v.get("error").and_then(Value::str).map(String::from),
            artifact: v.get("artifact").and_then(Value::str).map(String::from),
            retries: v
                .get("retries")
                .and_then(Value::integer)
                .and_then(|n| u8::try_from(n).ok())
                .ok_or("Invalid retry budget")?,
        })
    }
}
pub struct Coordinator {
    pub(crate) tasks: BTreeMap<u64, Task>,
    journal: Journal,
    since_snapshot: usize,
}
impl Coordinator {
    /// Call on a worker. Interrupted work is retained and requires an explicit retry.
    pub fn open(path: &Path) -> Result<Self, String> {
        let (journal, records) = Journal::open(path).map_err(|e| e.to_string())?;
        let mut this = Self {
            tasks: BTreeMap::new(),
            journal,
            since_snapshot: 0,
        };
        for (kind, bytes) in records {
            if kind != 10 {
                return Err("Unexpected task journal record".into());
            }
            let task = Task::parse(&json::parse(
                std::str::from_utf8(&bytes).map_err(|_| "Invalid task encoding")?,
            )?)?;
            this.tasks.insert(task.id, task);
        }
        if this.tasks.len() > 1000 {
            return Err("Task budget exceeded".into());
        }
        for task in this.tasks.values() {
            if task
                .dependencies
                .iter()
                .any(|id| *id >= task.id || !this.tasks.contains_key(id))
            {
                return Err("Invalid task dependency graph".into());
            }
        }
        let interrupted: Vec<_> = this
            .tasks
            .values()
            .filter(|t| matches!(t.state, State::Running | State::Validating))
            .cloned()
            .collect();
        for mut task in interrupted {
            task.state = State::Failed;
            task.error =
                Some("Interrupted by shutdown; inspect retained worktree before retrying".into());
            this.store(task)?;
        }
        Ok(this)
    }
    fn store(&mut self, task: Task) -> Result<(), String> {
        let value = task.value().encode();
        self.journal
            .append(10, value.as_bytes())
            .map_err(|e| e.to_string())?;
        self.tasks.insert(task.id, task);
        self.since_snapshot += 1;
        if self.since_snapshot >= 128 {
            let records = self
                .tasks
                .values()
                .map(|t| (10, t.value().encode().into_bytes()))
                .collect();
            self.journal
                .checkpoint(&records)
                .map_err(|e| e.to_string())?;
            self.since_snapshot = 0;
        }
        Ok(())
    }
    pub fn add(
        &mut self,
        workspace: PathBuf,
        objective: String,
        dependencies: Vec<u64>,
        validation: Vec<String>,
    ) -> Result<u64, String> {
        if self.tasks.len() >= 1000
            || objective.trim().is_empty()
            || objective.len() > 65536
            || validation.is_empty()
            || validation.len() > 256
            || validation.iter().map(String::len).sum::<usize>() > 16384
        {
            return Err("Invalid task or task budget exceeded".into());
        }
        if workspace.to_str().is_none() {
            return Err("Task persistence requires a UTF-8 workspace path".into());
        }
        if dependencies.iter().any(|id| !self.tasks.contains_key(id))
            || dependencies.iter().collect::<BTreeSet<_>>().len() != dependencies.len()
        {
            return Err("Dependencies must be unique existing tasks".into());
        }
        let id = self.tasks.last_key_value().map_or(1, |(&id, _)| id + 1);
        self.store(Task {
            id,
            objective,
            dependencies,
            state: State::Queued,
            workspace,
            worktree: None,
            base: None,
            session: None,
            validation,
            error: None,
            artifact: None,
            retries: 2,
        })?;
        self.refresh()?;
        Ok(id)
    }
    pub fn refresh(&mut self) -> Result<(), String> {
        let ready: Vec<_> = self
            .tasks
            .values()
            .filter(|t| {
                t.state == State::Queued
                    && t.dependencies
                        .iter()
                        .all(|id| self.tasks[id].state == State::Completed)
            })
            .cloned()
            .collect();
        for mut task in ready {
            task.state = State::Ready;
            self.store(task)?;
        }
        Ok(())
    }
    fn task(&self, id: u64) -> Result<Task, String> {
        self.tasks.get(&id).cloned().ok_or("Unknown task".into())
    }
    pub fn start(&mut self, id: u64, worktree: PathBuf, base: String) -> Result<(), String> {
        let mut task = self.task(id)?;
        if task.state != State::Ready {
            return Err("Task is not ready".into());
        }
        task.state = State::Running;
        task.worktree = Some(worktree);
        task.base = Some(base);
        task.error = None;
        self.store(task)
    }
    pub fn session(&mut self, id: u64, session: String) -> Result<(), String> {
        let mut task = self.task(id)?;
        task.session = Some(session);
        self.store(task)
    }
    pub fn agent_finished(&mut self, id: u64) -> Result<(), String> {
        let mut task = self.task(id)?;
        if task.state != State::Running {
            return Err("Task is not running".into());
        }
        task.state = State::Validating;
        self.store(task)
    }
    pub fn validated(&mut self, id: u64) -> Result<(), String> {
        let mut task = self.task(id)?;
        if task.state != State::Validating {
            return Err("Task is not validating".into());
        }
        task.state = State::AwaitingApproval;
        self.store(task)
    }
    pub fn fail(&mut self, id: u64, error: String) -> Result<(), String> {
        let mut task = self.task(id)?;
        if matches!(task.state, State::Completed | State::Cancelled) {
            return Err("Terminal task cannot fail".into());
        }
        task.state = State::Failed;
        task.error = Some(error.chars().take(8192).collect());
        self.store(task)
    }
    pub fn cancel(&mut self, id: u64) -> Result<(), String> {
        let mut task = self.task(id)?;
        if task.state == State::Completed {
            return Err("Completed task cannot cancel".into());
        }
        task.state = State::Cancelled;
        self.store(task)
    }
    pub fn retry(&mut self, id: u64) -> Result<(), String> {
        let mut task = self.task(id)?;
        if task.state != State::Failed || task.retries == 0 {
            return Err("Task cannot retry".into());
        }
        task.retries -= 1;
        task.state = State::Queued;
        task.error = None;
        self.store(task)?;
        self.refresh()
    }
    /// Only call after approved integration and successful combined validation.
    pub fn integrated(&mut self, id: u64, commit: String) -> Result<(), String> {
        let mut task = self.task(id)?;
        if task.state != State::AwaitingApproval || !valid_oid(&commit) {
            return Err("Task lacks approved, validated integration".into());
        }
        task.artifact = Some(commit);
        task.state = State::Completed;
        self.store(task)?;
        self.refresh()
    }
    pub fn report(&self) -> String {
        self.tasks
            .values()
            .map(|t| {
                format!(
                    "#{} {:?}: {}\n  worktree={}\n  session={}\n  {}\n",
                    t.id,
                    t.state,
                    t.objective,
                    t.worktree
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    t.session.as_deref().unwrap_or(""),
                    t.error.as_deref().unwrap_or("")
                )
            })
            .collect()
    }
}
pub fn valid_oid(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}
pub fn git(root: &Path, args: &[&str], cancel: &AtomicBool) -> Result<String, String> {
    let output = process::run(
        &process::git(root.to_owned(), args.iter().map(|s| (*s).into()).collect()),
        cancel,
    )
    .map_err(|e| e.to_string())?;
    if output.success() {
        Ok(output.stdout.trim().into())
    } else {
        Err(output.display())
    }
}
pub fn prepare(root: &Path, destination: &Path, cancel: &AtomicBool) -> Result<String, String> {
    if !git(root, &["status", "--porcelain"], cancel)?.is_empty() {
        return Err("Workspace must be clean before creating an agent worktree".into());
    }
    let base = git(root, &["rev-parse", "HEAD"], cancel)?;
    if !valid_oid(&base) {
        return Err("Invalid repository HEAD".into());
    }
    // Detached worktrees cannot accidentally move the user's branch. No destructive cleanup.
    git(
        root,
        &[
            "worktree",
            "add",
            "--detach",
            destination.to_str().ok_or("Non-UTF8 worktree path")?,
            &base,
        ],
        cancel,
    )?;
    Ok(base)
}
pub fn validate(task: &Task, root: &Path, cancel: &AtomicBool) -> Result<String, String> {
    let (program, args) = task
        .validation
        .split_first()
        .ok_or("Missing validation command")?;
    let result = process::run(
        &Spec {
            program: program.clone(),
            args: args.to_vec(),
            cwd: root.to_owned(),
            timeout: std::time::Duration::from_secs(300),
            capture_limit: 1024 * 1024,
        },
        cancel,
    )
    .map_err(|e| e.to_string())?;
    if result.success() {
        Ok(result.display())
    } else {
        Err(result.display())
    }
}
pub fn integrate(task: &Task, commit: &str, cancel: &AtomicBool) -> Result<String, String> {
    if task.state != State::AwaitingApproval || !valid_oid(commit) {
        return Err("Integration requires validated task and exact commit SHA".into());
    }
    let worktree = task.worktree.as_ref().ok_or("Task has no worktree")?;
    if git(worktree, &["rev-parse", "HEAD"], cancel)? != commit {
        return Err("Worktree HEAD changed; review again".into());
    }
    if !git(worktree, &["status", "--porcelain"], cancel)?.is_empty()
        || !git(&task.workspace, &["status", "--porcelain"], cancel)?.is_empty()
    {
        return Err("Both workspaces must be clean before integration".into());
    }
    let base = task.base.as_deref().ok_or("Missing task base")?;
    git(
        worktree,
        &["merge-base", "--is-ancestor", base, commit],
        cancel,
    )?;
    // Integrate all commits after the task base, not only the final commit.
    let range = format!("{base}..{commit}");
    let commits = git(
        worktree,
        &["rev-list", "--reverse", "--topo-order", &range],
        cancel,
    )?;
    let mut args = vec!["cherry-pick"];
    let revisions: Vec<_> = commits.lines().collect();
    if revisions.is_empty() || revisions.iter().any(|s| !valid_oid(s)) {
        return Err("No valid commits to integrate".into());
    }
    args.extend(revisions);
    git(&task.workspace, &args, cancel)?;
    validate(task, &task.workspace, cancel)?;
    git(&task.workspace, &["rev-parse", "HEAD"], cancel)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolated_worktree_integration_requires_validation() {
        let cancel = AtomicBool::new(false);
        let dir = std::env::temp_dir().join(format!("rivet-integration-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        let work = dir.join("writer");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init"], &cancel).unwrap();
        for (key, value) in [
            ("user.name", "Rivet test"),
            ("user.email", "test@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git(&repo, &["config", key, value], &cancel).unwrap();
        }
        std::fs::write(repo.join("source.txt"), "base\n").unwrap();
        git(&repo, &["add", "source.txt"], &cancel).unwrap();
        git(&repo, &["commit", "-m", "base"], &cancel).unwrap();
        let base = prepare(&repo, &work, &cancel).unwrap();
        std::fs::write(work.join("source.txt"), "agent implementation\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.join("source.txt")).unwrap(),
            "base\n"
        );
        git(&work, &["add", "source.txt"], &cancel).unwrap();
        git(&work, &["commit", "-m", "implementation"], &cancel).unwrap();
        let commit = git(&work, &["rev-parse", "HEAD"], &cancel).unwrap();
        let mut coordinator = Coordinator::open(&dir.join("tasks.journal")).unwrap();
        let py = if cfg!(windows) { "python" } else { "python3" };
        let id=coordinator.add(repo.clone(),"Implement".into(),vec![],vec![py.into(),"-c".into(),"import pathlib;assert 'agent implementation' in pathlib.Path('source.txt').read_text()".into()]).unwrap();
        coordinator.start(id, work.clone(), base).unwrap();
        assert!(integrate(&coordinator.tasks[&id], &commit, &cancel).is_err());
        coordinator.agent_finished(id).unwrap();
        validate(&coordinator.tasks[&id], &work, &cancel).unwrap();
        coordinator.validated(id).unwrap();
        assert!(integrate(&coordinator.tasks[&id], &"f".repeat(40), &cancel).is_err());
        let head = integrate(&coordinator.tasks[&id], &commit, &cancel).unwrap();
        coordinator.integrated(id, head).unwrap();
        assert_eq!(coordinator.tasks[&id].state, State::Completed);
        assert!(
            std::fs::read_to_string(repo.join("source.txt"))
                .unwrap()
                .contains("agent implementation")
        );
        drop(coordinator);
        git(
            &repo,
            &["worktree", "remove", work.to_str().unwrap()],
            &cancel,
        )
        .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn dependencies_validation_and_crash_recovery() {
        let dir = std::env::temp_dir().join(format!("rivet-tasks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("journal");
        let _ = std::fs::remove_file(&p);
        let mut c = Coordinator::open(&p).unwrap();
        let a = c
            .add(dir.clone(), "Build".into(), vec![], vec!["true".into()])
            .unwrap();
        let b = c
            .add(dir.clone(), "Review".into(), vec![a], vec!["true".into()])
            .unwrap();
        assert_eq!(c.tasks[&b].state, State::Queued);
        assert!(c.start(b, dir.clone(), "a".repeat(40)).is_err());
        c.start(a, dir.clone(), "a".repeat(40)).unwrap();
        assert!(c.integrated(a, "b".repeat(40)).is_err());
        c.agent_finished(a).unwrap();
        c.validated(a).unwrap();
        assert_eq!(c.tasks[&a].state, State::AwaitingApproval);
        c.integrated(a, "b".repeat(40)).unwrap();
        assert_eq!(c.tasks[&b].state, State::Ready);
        c.start(b, dir.clone(), "b".repeat(40)).unwrap();
        drop(c);
        let mut c = Coordinator::open(&p).unwrap();
        assert_eq!(c.tasks[&b].state, State::Failed);
        c.retry(b).unwrap();
        assert_eq!(c.tasks[&b].retries, 1);
        drop(c);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
