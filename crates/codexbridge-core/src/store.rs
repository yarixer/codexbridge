use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::{
    task::{Task, TaskSpec, TaskStatus},
    workspace::{Chat, Project},
};

pub struct TaskStore {
    connection: Mutex<Connection>,
}

impl TaskStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection =
            Connection::open(path).context("failed to open the CodexBridge database")?;
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
            );
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                path TEXT NOT NULL UNIQUE,
                remote_url TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chats (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                title TEXT NOT NULL,
                archived INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS chats_project_updated
                ON chats(project_id, archived, updated_at DESC);
            CREATE TABLE IF NOT EXISTS chat_tasks (
                chat_id TEXT PRIMARY KEY REFERENCES chats(id) ON DELETE CASCADE,
                task_id TEXT NOT NULL UNIQUE REFERENCES tasks(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
                role TEXT NOT NULL,
                kind TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS messages_chat_created
                ON messages(chat_id, created_at);",
        )?;
        migrate_legacy_tasks(&connection)?;
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
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
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
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
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
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
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
            bail!("invalid task transition: {expected} -> {next}");
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        let transaction = connection.transaction()?;
        let now = now_unix();
        let changed = transaction.execute(
            "UPDATE tasks SET status = ?1, updated_at = ?2 WHERE id = ?3 AND status = ?4",
            params![next.to_string(), now, id.to_string(), expected.to_string()],
        )?;
        if changed != 1 {
            bail!("task changed or does not exist; expected state {expected}");
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
        self.get_task(id)?.context("task disappeared after update")
    }

    pub fn increment_worker_attempts(&self, id: Uuid) -> Result<Task> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        let now = now_unix();
        let changed = connection.execute(
            "UPDATE tasks SET worker_attempts = worker_attempts + 1, updated_at = ?1 WHERE id = ?2",
            params![now, id.to_string()],
        )?;
        if changed != 1 {
            bail!("task does not exist")
        }
        drop(connection);
        self.get_task(id)?.context("task disappeared after update")
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
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
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
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        let payload = connection
            .query_row(
                "SELECT payload_json FROM task_artifacts WHERE task_id = ?1 AND artifact_type = ?2",
                params![task_id.to_string(), artifact_type],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        payload
            .map(|json| serde_json::from_str(&json).context("corrupted task artifact"))
            .transpose()
    }

    pub fn blocked_from(&self, task_id: Uuid) -> Result<Option<TaskStatus>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        let payload = connection
            .query_row(
                "SELECT payload_json FROM task_events
                 WHERE task_id = ?1 AND event_type = 'task.transitioned'
                 ORDER BY sequence DESC LIMIT 1",
                [task_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(payload) = payload else {
            return Ok(None);
        };
        let payload: serde_json::Value =
            serde_json::from_str(&payload).context("corrupted task transition event")?;
        if payload.get("to").and_then(serde_json::Value::as_str) != Some("blocked") {
            return Ok(None);
        }
        payload
            .get("from")
            .cloned()
            .map(|value| serde_json::from_value(value).context("unknown task blocking stage"))
            .transpose()
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        connection
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Into::into)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        connection.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, now_unix()],
        )?;
        Ok(())
    }

    pub fn upsert_project(
        &self,
        name: &str,
        path: &str,
        remote_url: Option<&str>,
    ) -> Result<Project> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        let now = now_unix();
        let id = Uuid::new_v4();
        connection.execute(
            "INSERT INTO projects (id, name, path, remote_url, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(path) DO UPDATE SET
               name = excluded.name,
               remote_url = COALESCE(excluded.remote_url, projects.remote_url),
               updated_at = excluded.updated_at",
            params![id.to_string(), name, path, remote_url, now],
        )?;
        connection
            .query_row(
                "SELECT id, name, path, remote_url, created_at, updated_at FROM projects WHERE path = ?1",
                [path],
                project_from_row,
            )
            .map_err(Into::into)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        let mut statement = connection.prepare(
            "SELECT id, name, path, remote_url, created_at, updated_at
             FROM projects ORDER BY updated_at DESC, name COLLATE NOCASE",
        )?;
        Ok(statement
            .query_map([], project_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_project(&self, id: Uuid) -> Result<Option<Project>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        connection
            .query_row(
                "SELECT id, name, path, remote_url, created_at, updated_at FROM projects WHERE id = ?1",
                [id.to_string()],
                project_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn create_chat(&self, project_id: Uuid, title: &str) -> Result<Chat> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        let id = Uuid::new_v4();
        let now = now_unix();
        let title = if title.trim().is_empty() {
            "New chat"
        } else {
            title.trim()
        };
        connection.execute(
            "INSERT INTO chats (id, project_id, title, archived, created_at, updated_at)
             VALUES (?1, ?2, ?3, 0, ?4, ?4)",
            params![id.to_string(), project_id.to_string(), title, now],
        )?;
        drop(connection);
        self.get_chat(id)?.context("chat not found after creation")
    }

    pub fn get_chat(&self, id: Uuid) -> Result<Option<Chat>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        connection
            .query_row(
                "SELECT c.id, c.project_id, c.title, c.archived, c.created_at, c.updated_at,
                        ct.task_id, t.status
                 FROM chats c
                 LEFT JOIN chat_tasks ct ON ct.chat_id = c.id
                 LEFT JOIN tasks t ON t.id = ct.task_id
                 WHERE c.id = ?1",
                [id.to_string()],
                chat_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_chats(&self, project_id: Uuid, archived: bool) -> Result<Vec<Chat>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        let mut statement = connection.prepare(
            "SELECT c.id, c.project_id, c.title, c.archived, c.created_at, c.updated_at,
                    ct.task_id, t.status
             FROM chats c
             LEFT JOIN chat_tasks ct ON ct.chat_id = c.id
             LEFT JOIN tasks t ON t.id = ct.task_id
             WHERE c.project_id = ?1 AND c.archived = ?2
             ORDER BY c.updated_at DESC",
        )?;
        Ok(statement
            .query_map(params![project_id.to_string(), archived], chat_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn archive_chat(&self, id: Uuid, archived: bool) -> Result<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        let changed = connection.execute(
            "UPDATE chats SET archived = ?1, updated_at = ?2 WHERE id = ?3",
            params![archived, now_unix(), id.to_string()],
        )?;
        if changed != 1 {
            bail!("chat not found")
        }
        Ok(())
    }

    pub fn link_task_to_chat(&self, chat_id: Uuid, task_id: Uuid, title: &str) -> Result<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        let now = now_unix();
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO chat_tasks (chat_id, task_id) VALUES (?1, ?2)
             ON CONFLICT(chat_id) DO UPDATE SET task_id = excluded.task_id",
            params![chat_id.to_string(), task_id.to_string()],
        )?;
        transaction.execute(
            "UPDATE chats SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title, now, chat_id.to_string()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn task_for_chat(&self, chat_id: Uuid) -> Result<Option<Uuid>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("task database is locked"))?;
        let id = connection
            .query_row(
                "SELECT task_id FROM chat_tasks WHERE chat_id = ?1",
                [chat_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        id.map(|id| Uuid::parse_str(&id).context("corrupted chat task id"))
            .transpose()
    }
}

fn migrate_legacy_tasks(connection: &Connection) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT t.id, t.spec_json, t.created_at, t.updated_at
         FROM tasks t LEFT JOIN chat_tasks ct ON ct.task_id = t.id
         WHERE ct.task_id IS NULL ORDER BY t.created_at",
    )?;
    let legacy = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for (task_id, spec_json, created_at, updated_at) in legacy {
        let spec: TaskSpec = serde_json::from_str(&spec_json)?;
        let project_id = connection
            .query_row(
                "SELECT id FROM projects WHERE path = ?1",
                [&spec.workspace],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let name = std::path::Path::new(&spec.workspace)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Repository");
        connection.execute(
            "INSERT OR IGNORE INTO projects (id, name, path, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project_id, name, spec.workspace, created_at, updated_at],
        )?;
        let chat_id = Uuid::new_v4().to_string();
        connection.execute(
            "INSERT INTO chats (id, project_id, title, archived, created_at, updated_at)
             VALUES (?1, ?2, ?3, 0, ?4, ?5)",
            params![
                chat_id,
                project_id,
                short_title(&spec.goal),
                created_at,
                updated_at
            ],
        )?;
        connection.execute(
            "INSERT INTO chat_tasks (chat_id, task_id) VALUES (?1, ?2)",
            params![chat_id, task_id],
        )?;
    }
    Ok(())
}

fn short_title(value: &str) -> String {
    let value = value.trim().replace(['\r', '\n'], " ");
    let mut title = value.chars().take(64).collect::<String>();
    if value.chars().count() > 64 {
        title.push('…');
    }
    if title.is_empty() {
        "New chat".into()
    } else {
        title
    }
}

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: parse_uuid_column(row, 0)?,
        name: row.get(1)?,
        path: row.get(2)?,
        remote_url: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn chat_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Chat> {
    let task_id = row
        .get::<_, Option<String>>(6)?
        .map(|id| Uuid::parse_str(&id).map_err(|error| conversion_error(6, error)))
        .transpose()?;
    let task_status = row
        .get::<_, Option<String>>(7)?
        .map(|status| {
            serde_json::from_str(&format!("\"{status}\""))
                .map_err(|error| conversion_error(7, error))
        })
        .transpose()?;
    Ok(Chat {
        id: parse_uuid_column(row, 0)?,
        project_id: parse_uuid_column(row, 1)?,
        title: row.get(2)?,
        archived: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        task_id,
        task_status,
    })
}

fn parse_uuid_column(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Uuid> {
    let value: String = row.get(index)?;
    Uuid::parse_str(&value).map_err(|error| conversion_error(index, error))
}

fn conversion_error(
    index: usize,
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(error))
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
            goal: "Fix the bug".into(),
            workspace: "C:/repo".into(),
            base_commit: "abc123".into(),
            constraints: vec!["Preserve the API".into()],
            acceptance_criteria: vec!["The test passes".into()],
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
        store
            .transition(
                task.id,
                TaskStatus::Planning,
                TaskStatus::Blocked,
                Some("error"),
            )
            .unwrap();
        assert_eq!(
            store.blocked_from(task.id).unwrap(),
            Some(TaskStatus::Planning)
        );
        assert!(
            store
                .transition(task.id, TaskStatus::Draft, TaskStatus::Planning, None)
                .is_err()
        );
        assert_eq!(store.list_tasks().unwrap().len(), 1);

        assert_eq!(store.get_setting("workspace").unwrap(), None);
        store.set_setting("workspace", "C:/repo").unwrap();
        assert_eq!(
            store.get_setting("workspace").unwrap().as_deref(),
            Some("C:/repo")
        );

        let project = store
            .upsert_project("repo", "C:/repo", Some("https://example.com/repo.git"))
            .unwrap();
        assert_eq!(store.list_projects().unwrap(), vec![project.clone()]);
        let chat = store.create_chat(project.id, "Fix the bug").unwrap();
        store.link_task_to_chat(chat.id, task.id, "Task").unwrap();
        assert_eq!(store.task_for_chat(chat.id).unwrap(), Some(task.id));
        assert_eq!(store.list_chats(project.id, false).unwrap().len(), 1);
        store.archive_chat(chat.id, true).unwrap();
        assert!(store.list_chats(project.id, false).unwrap().is_empty());
        assert_eq!(store.list_chats(project.id, true).unwrap().len(), 1);

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
