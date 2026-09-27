use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    codex::{self, ArchitectReview},
    cursor::{self, CursorRunResult},
    git,
    store::TaskStore,
    task::{ArchitectPlan, Task, TaskSpec, TaskStatus},
    validation::{self, ValidationResult},
};

const PLAN_ARTIFACT: &str = "architect.plan";
const WORKTREE_ARTIFACT: &str = "git.worktree";
const WORKER_ARTIFACT: &str = "cursor.run";
const VALIDATION_ARTIFACT: &str = "validation.results";
const REVIEW_ARTIFACT: &str = "architect.review";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskRequest {
    pub goal: String,
    pub workspace: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub validation_commands: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSession {
    pub task: Task,
    pub blocked_from: Option<TaskStatus>,
    pub plan: Option<ArchitectPlan>,
    pub worktree: Option<String>,
    pub worker: Option<CursorRunResult>,
    pub validation: Vec<ValidationResult>,
    pub review: Option<ArchitectReview>,
}

pub async fn create_and_plan(store: &TaskStore, request: CreateTaskRequest) -> Result<TaskSession> {
    let repository = git::inspect(&request.workspace).await?;
    if repository.dirty {
        bail!(
            "The workspace contains uncommitted changes. Commit or stash them first so Astra and Grok work from the same baseline."
        );
    }
    let spec = TaskSpec {
        goal: request.goal,
        workspace: repository.root,
        base_commit: repository.head,
        constraints: request.constraints,
        acceptance_criteria: request.acceptance_criteria,
        validation_commands: request.validation_commands,
        max_worker_attempts: 2,
    };
    let task = store.create_task(spec)?;
    let task = store.transition(task.id, TaskStatus::Draft, TaskStatus::Planning, None)?;
    match codex::plan_task(&task.spec).await {
        Ok(plan) => {
            store.save_artifact(task.id, PLAN_ARTIFACT, &plan)?;
            load_session(store, task.id)
        }
        Err(error) => {
            let _ = store.transition(
                task.id,
                TaskStatus::Planning,
                TaskStatus::Blocked,
                Some(&error.to_string()),
            );
            Err(error)
        }
    }
}

pub async fn execute_task(
    store: &TaskStore,
    task_id: Uuid,
    cursor_api_key: Option<&str>,
    cursor_bridge_binary: Option<&Path>,
    worktree_root: &Path,
    cursor_state_root: &Path,
) -> Result<TaskSession> {
    let task = store.get_task(task_id)?.context("task not found")?;
    let start_stage = match task.status {
        TaskStatus::Planning | TaskStatus::NeedsRevision => TaskStatus::Executing,
        TaskStatus::Blocked => match store.blocked_from(task_id)? {
            Some(TaskStatus::Validating) => TaskStatus::Validating,
            Some(TaskStatus::Reviewing) => TaskStatus::Reviewing,
            _ => TaskStatus::Executing,
        },
        status => return Err(anyhow!("a task in state {status} cannot be started")),
    };
    if task.worker_attempts >= task.spec.max_worker_attempts && task.status != TaskStatus::Blocked {
        return Err(anyhow!("Grok attempt limit exhausted"));
    }
    let plan = store
        .load_artifact::<ArchitectPlan>(task_id, PLAN_ARTIFACT)?
        .context("Astra plan not found")?;
    let cursor_api_key = if start_stage == TaskStatus::Executing {
        Some(
            cursor_api_key
                .filter(|value| !value.trim().is_empty())
                .context("Cursor API key is not configured")?,
        )
    } else {
        None
    };
    let worktree = git::create_worktree(
        &task.spec.workspace,
        &task.spec.base_commit,
        worktree_root,
        task_id,
    )
    .await?;
    store.save_artifact(task_id, WORKTREE_ARTIFACT, &worktree)?;
    if start_stage == TaskStatus::Executing {
        store.transition(task_id, task.status, TaskStatus::Executing, None)?;
        let previous_review = store.load_artifact::<ArchitectReview>(task_id, REVIEW_ARTIFACT)?;
        let prompt = worker_prompt(&task.spec, &plan, previous_review.as_ref())?;
        let task_cursor_state_root = cursor_state_root.join(task_id.to_string());
        let worker = match cursor::execute(
            &worktree,
            cursor_api_key.expect("key checked for the execution stage"),
            cursor_bridge_binary,
            &task_cursor_state_root,
            &prompt,
        )
        .await
        {
            Ok(worker) => worker,
            Err(error) => {
                let _ = store.transition(
                    task_id,
                    TaskStatus::Executing,
                    TaskStatus::Blocked,
                    Some(&error.to_string()),
                );
                return Err(error);
            }
        };
        store.increment_worker_attempts(task_id)?;
        store.save_artifact(task_id, WORKER_ARTIFACT, &worker)?;
        store.transition(task_id, TaskStatus::Executing, TaskStatus::Validating, None)?;
    } else {
        store.transition(task_id, TaskStatus::Blocked, start_stage, None)?;
    }

    let validation = if start_stage == TaskStatus::Reviewing {
        store
            .load_artifact::<Vec<ValidationResult>>(task_id, VALIDATION_ARTIFACT)?
            .context("validation results not found")?
    } else {
        let validation = match validation::run_all(&worktree, &task.spec.validation_commands).await
        {
            Ok(validation) => validation,
            Err(error) => {
                let _ = store.transition(
                    task_id,
                    TaskStatus::Validating,
                    TaskStatus::Blocked,
                    Some(&error.to_string()),
                );
                return Err(error);
            }
        };
        store.save_artifact(task_id, VALIDATION_ARTIFACT, &validation)?;
        if validation.iter().any(|result| !result.success) {
            store.transition(
                task_id,
                TaskStatus::Validating,
                TaskStatus::NeedsRevision,
                Some("one or more validation commands failed"),
            )?;
            return load_session(store, task_id);
        }
        store.transition(task_id, TaskStatus::Validating, TaskStatus::Reviewing, None)?;
        validation
    };
    let diff = match git::diff(&worktree).await {
        Ok(diff) => diff,
        Err(error) => {
            let _ = store.transition(
                task_id,
                TaskStatus::Reviewing,
                TaskStatus::Blocked,
                Some(&error.to_string()),
            );
            return Err(error);
        }
    };
    let validation_summary = validation
        .iter()
        .map(|result| {
            format!(
                "{}: {}\n{}",
                result.command.join(" "),
                if result.success { "PASS" } else { "FAIL" },
                result.output
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let review_workspace = worktree
        .to_str()
        .context("worktree path contains unsupported characters")?;
    let review = match codex::review_task(
        review_workspace,
        &task.spec,
        &plan,
        &diff,
        &validation_summary,
    )
    .await
    {
        Ok(review) => review,
        Err(error) => {
            let _ = store.transition(
                task_id,
                TaskStatus::Reviewing,
                TaskStatus::Blocked,
                Some(&error.to_string()),
            );
            return Err(error);
        }
    };
    store.save_artifact(task_id, REVIEW_ARTIFACT, &review)?;
    store.transition(
        task_id,
        TaskStatus::Reviewing,
        if review.approved {
            TaskStatus::Completed
        } else {
            TaskStatus::NeedsRevision
        },
        Some(&review.summary),
    )?;
    load_session(store, task_id)
}

pub fn load_session(store: &TaskStore, task_id: Uuid) -> Result<TaskSession> {
    let task = store.get_task(task_id)?.context("task not found")?;
    let blocked_from = if task.status == TaskStatus::Blocked {
        store.blocked_from(task_id)?
    } else {
        None
    };
    Ok(TaskSession {
        task,
        blocked_from,
        plan: store.load_artifact(task_id, PLAN_ARTIFACT)?,
        worktree: store
            .load_artifact::<PathBuf>(task_id, WORKTREE_ARTIFACT)?
            .map(|path| path.to_string_lossy().into_owned()),
        worker: store.load_artifact(task_id, WORKER_ARTIFACT)?,
        validation: store
            .load_artifact(task_id, VALIDATION_ARTIFACT)?
            .unwrap_or_default(),
        review: store.load_artifact(task_id, REVIEW_ARTIFACT)?,
    })
}

pub fn recover_interrupted_tasks(store: &TaskStore) -> Result<usize> {
    let mut recovered = 0;
    for task in store.list_tasks()? {
        let interrupted = match task.status {
            TaskStatus::Executing | TaskStatus::Validating | TaskStatus::Reviewing => true,
            TaskStatus::Planning => store
                .load_artifact::<ArchitectPlan>(task.id, PLAN_ARTIFACT)?
                .is_none(),
            _ => false,
        };
        if interrupted {
            store.transition(
                task.id,
                task.status,
                TaskStatus::Blocked,
                Some("the previous CodexBridge process exited during execution"),
            )?;
            recovered += 1;
        }
    }
    Ok(recovered)
}

fn worker_prompt(
    spec: &TaskSpec,
    plan: &ArchitectPlan,
    previous_review: Option<&ArchitectReview>,
) -> Result<String> {
    let revision = previous_review.map_or_else(String::new, |review| {
        format!(
            "\n\nIssues from the previous review that must be fixed:\n{}",
            review.issues.join("\n")
        )
    });
    Ok(format!(
        "You are the CodexBridge implementation agent. Implement the task in the current Git worktree. Modify files and run the necessary local checks. Do not create a commit or modify other checkouts. Do not stop at an explanation: bring the code to a working state.\n\nGoal:\n{}\n\nConstraints:\n{}\n\nAcceptance criteria:\n{}\n\nApproved Astra plan:\n{}{}",
        spec.goal,
        spec.constraints.join("\n"),
        spec.acceptance_criteria.join("\n"),
        serde_json::to_string_pretty(plan)?,
        revision
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> TaskSpec {
        TaskSpec {
            goal: "test".into(),
            workspace: "C:/repo".into(),
            base_commit: "abc123".into(),
            constraints: vec![],
            acceptance_criteria: vec![],
            validation_commands: vec![],
            max_worker_attempts: 2,
        }
    }

    fn plan() -> ArchitectPlan {
        ArchitectPlan {
            summary: "plan".into(),
            work_items: vec![],
            risks: vec![],
        }
    }

    #[test]
    fn recovers_only_interrupted_active_tasks() {
        let directory = tempfile::tempdir().unwrap();
        let store = TaskStore::open(directory.path().join("state.db")).unwrap();
        let interrupted = store.create_task(spec()).unwrap();
        store
            .transition(
                interrupted.id,
                TaskStatus::Draft,
                TaskStatus::Planning,
                None,
            )
            .unwrap();
        store
            .save_artifact(interrupted.id, PLAN_ARTIFACT, &plan())
            .unwrap();
        store
            .transition(
                interrupted.id,
                TaskStatus::Planning,
                TaskStatus::Executing,
                None,
            )
            .unwrap();
        let ready = store.create_task(spec()).unwrap();
        store
            .transition(ready.id, TaskStatus::Draft, TaskStatus::Planning, None)
            .unwrap();
        store
            .save_artifact(ready.id, PLAN_ARTIFACT, &plan())
            .unwrap();

        assert_eq!(recover_interrupted_tasks(&store).unwrap(), 1);
        assert_eq!(
            store.get_task(interrupted.id).unwrap().unwrap().status,
            TaskStatus::Blocked
        );
        assert_eq!(
            store.get_task(ready.id).unwrap().unwrap().status,
            TaskStatus::Planning
        );
    }
}
