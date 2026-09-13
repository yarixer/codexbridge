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
            "Рабочая папка содержит незакоммиченные изменения. Сначала создай commit или stash, чтобы Astra и Grok работали от одной базы."
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
    cursor_api_key: &str,
    cursor_bridge_binary: Option<&Path>,
    worktree_root: &Path,
) -> Result<TaskSession> {
    let task = store.get_task(task_id)?.context("задача не найдена")?;
    let expected = match task.status {
        TaskStatus::Planning => TaskStatus::Planning,
        TaskStatus::NeedsRevision => TaskStatus::NeedsRevision,
        TaskStatus::Blocked => TaskStatus::Blocked,
        status => return Err(anyhow!("задачу в состоянии {status} нельзя запустить")),
    };
    if task.worker_attempts >= task.spec.max_worker_attempts {
        return Err(anyhow!("исчерпан лимит попыток Grok"));
    }
    let plan = store
        .load_artifact::<ArchitectPlan>(task_id, PLAN_ARTIFACT)?
        .context("план Astra не найден")?;
    let worktree = git::create_worktree(
        &task.spec.workspace,
        &task.spec.base_commit,
        worktree_root,
        task_id,
    )
    .await?;
    store.save_artifact(task_id, WORKTREE_ARTIFACT, &worktree)?;
    store.transition(task_id, expected, TaskStatus::Executing, None)?;
    let task = store.increment_worker_attempts(task_id)?;
    let previous_review = store.load_artifact::<ArchitectReview>(task_id, REVIEW_ARTIFACT)?;
    let prompt = worker_prompt(&task.spec, &plan, previous_review.as_ref())?;
    let worker =
        match cursor::execute(&worktree, cursor_api_key, cursor_bridge_binary, &prompt).await {
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
    store.save_artifact(task_id, WORKER_ARTIFACT, &worker)?;
    store.transition(task_id, TaskStatus::Executing, TaskStatus::Validating, None)?;
    let validation = match validation::run_all(&worktree, &task.spec.validation_commands).await {
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
            Some("одна или несколько команд проверки завершились ошибкой"),
        )?;
        return load_session(store, task_id);
    }

    store.transition(task_id, TaskStatus::Validating, TaskStatus::Reviewing, None)?;
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
        .context("путь worktree содержит неподдерживаемые символы")?;
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
    Ok(TaskSession {
        task: store.get_task(task_id)?.context("задача не найдена")?,
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

fn worker_prompt(
    spec: &TaskSpec,
    plan: &ArchitectPlan,
    previous_review: Option<&ArchitectReview>,
) -> Result<String> {
    let revision = previous_review.map_or_else(String::new, |review| {
        format!(
            "\n\nЗамечания предыдущего ревью, которые нужно исправить:\n{}",
            review.issues.join("\n")
        )
    });
    Ok(format!(
        "Ты исполнитель Yarocursor. Реализуй задачу в текущем Git worktree. Изменяй файлы и запускай нужные локальные проверки. Не создавай commit и не меняй другие checkout. Не ограничивайся объяснением: доведи код до рабочего состояния.\n\nЦель:\n{}\n\nОграничения:\n{}\n\nКритерии приёмки:\n{}\n\nУтверждённый план Astra:\n{}{}",
        spec.goal,
        spec.constraints.join("\n"),
        spec.acceptance_criteria.join("\n"),
        serde_json::to_string_pretty(plan)?,
        revision
    ))
}
