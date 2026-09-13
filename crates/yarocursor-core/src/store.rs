use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::task::{Task, TaskSpec, TaskStatus};

pub struct TaskStore {
    connection: Mutex<Connection>,
}

impl TaskStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path).context("не удалось открыть базу Yarocursor")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                spec_json TEXT NOT NULL,
                status TEXT NOT NULL,
                worker_attempts INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS task_events (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                event_type TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS task_events_task_sequence
                ON task_events(task_id, sequence);
            CREATE TABLE IF NOT EXISTS task_artifacts (
                task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                artifact_type TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY (task_id, artifact_type)
            );",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn create_task(&self, spec: TaskSpec) -> Result<Task> {
        spec.validate().map_err(anyhow::Error::msg)?;
        let task = Task {
            id: Uuid::new_v4(),
            spec,
            status: TaskStatus::Draft,
            worker_attempts: 0,
            created_at: now_unix(),
            updated_at: now_unix(),
        };
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("база задач заблокирована"))?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO tasks (id, spec_json, status, worker_attempts, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                task.id.to_string(),
                serde_json::to_string(&task.spec)?,
                task.status.to_string(),
                task.worker_attempts,
                task.created_at,
                task.updated_at
            ],
        )?;
        append_event(
            &transaction,
            task.id,
            "task.created",
            serde_json::to_string(&task)?,
            task.created_at,
        )?;
        transaction.commit()?;
        Ok(task)
    }

    pub fn get_task(&self, id: Uuid) -> Result<Option<Task>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("база задач заблокирована"))?;
        connection.query_row(
            "SELECT id, spec_json, status, worker_attempts, created_at, updated_at FROM tasks WHERE id = ?1",
            [id.to_string()],
            task_from_row,
        ).optional().map_err(Into::into)
    }

    pub fn list_tasks(&self) -> Result<Vec<Task>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("база задач заблокирована"))?;
        let mut statement = connection.prepare(
            "SELECT id, spec_json, status, worker_attempts, created_at, updated_at FROM tasks ORDER BY updated_at DESC"
        )?;
        Ok(statement
            .query_map([], task_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn transition(
        &self,
        id: Uuid,
        expected: TaskStatus,
        next: TaskStatus,
        reason: Option<&str>,
    ) -> Result<Task> {
        if !expected.can_transition_to(next) {
            bail!("недопустимый переход задачи: {expected} -> {next}");
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("база задач заблокирована"))?;
        let transaction = connection.transaction()?;
        let now = now_unix();
        let changed = transaction.execute(
            "UPDATE tasks SET status = ?1, updated_at = ?2 WHERE id = ?3 AND status = ?4",
            params![next.to_string(), now, id.to_string(), expected.to_string()],
        )?;
        if changed != 1 {
            bail!("задача изменилась или не существует; ожидалось состояние {expected}");
        }
        append_event(
            &transaction,
            id,
            "task.transitioned",
            serde_json::json!({
                "from": expected,
                "to": next,
                "reason": reason,
            })
            .to_string(),
            now,
        )?;
        transaction.commit()?;
        drop(connection);
        self.get_task(id)?
            .context("задача исчезла после обновления")
    }

    pub fn increment_worker_attempts(&self, id: Uuid) -> Result<Task> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("база задач заблокирована"))?;
        let now = now_unix();
        let changed = connection.execute(
            "UPDATE tasks SET worker_attempts = worker_attempts + 1, updated_at = ?1 WHERE id = ?2",
            params![now, id.to_string()],
        )?;
        if changed != 1 {
            bail!("задача не существует")
        }
        drop(connection);
        self.get_task(id)?
            .context("задача исчезла после обновления")
    }

    pub fn save_artifact<T: Serialize>(
        &self,
        task_id: Uuid,
        artifact_type: &str,
        value: &T,
    ) -> Result<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("база задач заблокирована"))?;
        connection.execute(
            "INSERT INTO task_artifacts (task_id, artifact_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(task_id, artifact_type) DO UPDATE SET
               payload_json = excluded.payload_json,
               created_at = excluded.created_at",
            params![
                task_id.to_string(),
                artifact_type,
                serde_json::to_string(value)?,
                now_unix()
            ],
        )?;
        Ok(())
    }

    pub fn load_artifact<T: DeserializeOwned>(
        &self,
        task_id: Uuid,
        artifact_type: &str,
    ) -> Result<Option<T>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("база задач заблокирована"))?;
        let payload = connection
            .query_row(
                "SELECT payload_json FROM task_artifacts WHERE task_id = ?1 AND artifact_type = ?2",
                params![task_id.to_string(), artifact_type],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        payload
            .map(|json| serde_json::from_str(&json).context("повреждённый артефакт задачи"))
            .transpose()
    }
}

fn append_event(
    connection: &Connection,
    task_id: Uuid,
    event_type: &str,
    payload: String,
    created_at: i64,
) -> Result<()> {
    connection.execute(
        "INSERT INTO task_events (task_id, event_type, payload_json, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![task_id.to_string(), event_type, payload, created_at],
    )?;
    Ok(())
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let id: String = row.get(0)?;
    let spec_json: String = row.get(1)?;
    let status_json: String = row.get(2)?;
    Ok(Task {
        id: Uuid::parse_str(&id).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        spec: serde_json::from_str(&spec_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        status: serde_json::from_str(&format!("\"{status_json}\"")).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        worker_attempts: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> TaskSpec {
        TaskSpec {
            goal: "Исправить ошибку".into(),
            workspace: "C:/repo".into(),
            base_commit: "abc123".into(),
            constraints: vec!["Сохранить API".into()],
            acceptance_criteria: vec!["Тест проходит".into()],
            validation_commands: vec![vec!["cargo".into(), "test".into()]],
            max_worker_attempts: 2,
        }
    }

    #[test]
    fn persists_task_and_enforces_optimistic_transition() {
        let directory = tempfile::tempdir().unwrap();
        let store = TaskStore::open(directory.path().join("state.db")).unwrap();
        let task = store.create_task(spec()).unwrap();
        assert_eq!(store.get_task(task.id).unwrap().unwrap(), task);

        let planned = store
            .transition(task.id, TaskStatus::Draft, TaskStatus::Planning, None)
            .unwrap();
        assert_eq!(planned.status, TaskStatus::Planning);
        assert!(
            store
                .transition(task.id, TaskStatus::Draft, TaskStatus::Planning, None)
                .is_err()
        );
        assert_eq!(store.list_tasks().unwrap().len(), 1);

        store
            .save_artifact(task.id, "answer", &serde_json::json!({ "ok": true }))
            .unwrap();
        assert_eq!(
            store
                .load_artifact::<serde_json::Value>(task.id, "answer")
                .unwrap(),
            Some(serde_json::json!({ "ok": true }))
        );
    }
}
