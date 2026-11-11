# Stage 7: task supervision

The native Tasks pane accepts an objective, dependency IDs, and an executable validation
command as a JSON array. Add tasks, then Run ready tasks. The coordinator persists every
transition before acting, schedules up to three SDK sessions, and creates a detached Git
worktree per writing attempt. Interactive conversation turns are serialized in their
SDK host; parallel turns are enabled only for the task coordinator using these isolated
worktrees. Switching among persistent interactive sessions remains available during work. Agent behavior remains in the official SDK; the coordinator
handles dependency state, isolation, validation, and approvals.

A finished agent turn enters validation, not Completed. Passing validation waits for human
review. Inspect the worktree shown in the task report, review and commit its changes there,
then enter its exact HEAD SHA and task ID. Integrate / Confirm previews the integration;
a second click confirms that same task and SHA. Both workspaces must be clean. All task
commits since its recorded base are cherry-picked, followed by combined validation.
Only a successful integration and validation marks Completed and releases dependents.

Tool requests have separate allow-once/reject controls in Tasks. Approval is not sandboxing.
Cancellation stops scheduling and requests SDK abort; each attempt keeps its worktree.
Interrupted Running/Validating tasks recover as Failed for inspection, with two explicit
retries. Queued tasks retain dependencies. Worktrees are never silently deleted or reset.
Conflicts or failed combined validation retain the main workspace state for inspection.
No push, deployment, automatic dependency installation, or automatic commits are performed
by the coordinator. Agents still require permission through SDK hooks.

Acceptance still open: live multi-agent/model verification (model access currently fails),
conflict resolution UI, per-task cancellation during a blocking validation, task-scoped
permission expiration cleanup, full artifact browsing, and automated cleanup of archived
sessions. Git preparation/validation execute serially on a dedicated coordinator worker;
the UI remains independent, but long validation can delay other task permissions (which
expire denied). The SDK host currently bounds resident sessions at 20. These limitations
must be addressed before claiming production orchestration at scale.

Tests use real Git repositories to prove isolated writes and exact-commit integration;
state tests prove dependency gating, validation gating, retry budgets, and crash recovery.
