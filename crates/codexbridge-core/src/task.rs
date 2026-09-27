use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskSpec {
    pub goal: String,
    pub workspace: String,
    pub base_commit: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub validation_commands: Vec<Vec<String>>,
    #[serde(default = "default_max_worker_attempts")]
    pub max_worker_attempts: u8,
}

fn default_max_worker_attempts() -> u8 {
    2
}

impl TaskSpec {
    pub fn validate(&self) -> Result<(), String> {
        if self.goal.trim().is_empty() {
            return Err("Task goal cannot be empty".into());
        }
        if self.workspace.trim().is_empty() {
            return Err("Workspace cannot be empty".into());
        }
        if self.base_commit.trim().is_empty() {
            return Err("Base commit cannot be empty".into());
        }
        if self.max_worker_attempts == 0 {
            return Err("At least one implementation attempt is required".into());
        }
        if self.validation_commands.iter().any(Vec::is_empty) {
            return Err("Validation command cannot be empty".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Draft,
    Planning,
    Executing,
    Validating,
    Reviewing,
    NeedsRevision,
    Completed,
    Blocked,
    Cancelled,
}

impl TaskStatus {
    pub fn can_transition_to(self, next: Self) -> bool {
        use TaskStatus::*;
        matches!(
            (self, next),
            (Draft, Planning)
                | (Planning, Executing | Blocked | Cancelled)
                | (Executing, Validating | Blocked | Cancelled)
                | (Validating, Reviewing | NeedsRevision | Blocked | Cancelled)
                | (Reviewing, Completed | NeedsRevision | Blocked | Cancelled)
                | (NeedsRevision, Executing | Blocked | Cancelled)
                | (
                    Blocked,
                    Planning | Executing | Validating | Reviewing | Cancelled
                )
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .unwrap_or_else(|| "unknown".into());
        formatter.write_str(&value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: Uuid,
    pub spec: TaskSpec,
    pub status: TaskStatus,
    pub worker_attempts: u8,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkItem {
    pub id: String,
    pub objective: String,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArchitectPlan {
    pub summary: String,
    pub work_items: Vec<WorkItem>,
    #[serde(default)]
    pub risks: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_rejects_skipping_validation() {
        assert!(TaskStatus::Draft.can_transition_to(TaskStatus::Planning));
        assert!(!TaskStatus::Executing.can_transition_to(TaskStatus::Completed));
        assert!(TaskStatus::Reviewing.can_transition_to(TaskStatus::Completed));
        assert!(TaskStatus::Blocked.can_transition_to(TaskStatus::Validating));
        assert!(!TaskStatus::Completed.can_transition_to(TaskStatus::Executing));
    }
}
