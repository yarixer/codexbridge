mod credentials;

use std::{collections::HashSet, path::PathBuf, sync::RwLock};

use secrecy::{ExposeSecret, SecretString};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;
use yarocursor_core::store::TaskStore;

const WORKSPACE_SETTING: &str = "workspace";

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    workspace: String,
    theme: String,
    active_project_id: Option<String>,
    active_chat_id: Option<String>,
}

struct AppState {
    cursor_api_key: RwLock<Option<SecretString>>,
    cursor_credential_store: Option<credentials::CursorCredentialStore>,
    credential_error: RwLock<Option<String>>,
    codex_login: tokio::sync::Mutex<Option<yarocursor_core::codex::CodexClient>>,
    cursor_bridge_binary: Option<PathBuf>,
    task_store: TaskStore,
    worktree_root: PathBuf,
    cursor_state_root: PathBuf,
    active_tasks: tokio::sync::Mutex<HashSet<Uuid>>,
}

impl AppState {
    fn new(
        cursor_api_key: Option<SecretString>,
        cursor_credential_store: Option<credentials::CursorCredentialStore>,
        credential_error: Option<String>,
        cursor_bridge_binary: Option<PathBuf>,
        task_store: TaskStore,
        worktree_root: PathBuf,
        cursor_state_root: PathBuf,
    ) -> Self {
        Self {
            cursor_api_key: RwLock::new(cursor_api_key),
            cursor_credential_store,
            credential_error: RwLock::new(credential_error),
            codex_login: tokio::sync::Mutex::new(None),
            cursor_bridge_binary,
            task_store,
            worktree_root,
            cursor_state_root,
            active_tasks: tokio::sync::Mutex::new(HashSet::new()),
        }
    }
}

#[tauri::command]
fn cursor_credential_status(
    state: State<'_, AppState>,
) -> Result<credentials::CredentialStatus, String> {
    credential_status(&state)
}

#[tauri::command]
fn set_cursor_api_key(
    value: String,
    state: State<'_, AppState>,
) -> Result<credentials::CredentialStatus, String> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        return credential_status(&state);
    }
    let store = state.cursor_credential_store.as_ref().ok_or_else(|| {
        state
            .credential_error
            .read()
            .ok()
            .and_then(|error| error.clone())
            .unwrap_or_else(|| "Системное хранилище секретов недоступно".into())
    })?;
    store.save(&value)?;
    let mut key = state
        .cursor_api_key
        .write()
        .map_err(|_| "Не удалось заблокировать настройки")?;
    *key = Some(SecretString::from(value));
    drop(key);
    *state
        .credential_error
        .write()
        .map_err(|_| "Не удалось обновить состояние хранилища")? = None;
    credential_status(&state)
}

#[tauri::command]
fn delete_cursor_api_key(
    state: State<'_, AppState>,
) -> Result<credentials::CredentialStatus, String> {
    let store = state
        .cursor_credential_store
        .as_ref()
        .ok_or("Системное хранилище секретов недоступно")?;
    store.delete()?;
    *state
        .cursor_api_key
        .write()
        .map_err(|_| "Не удалось заблокировать настройки")? = None;
    credential_status(&state)
}

fn credential_status(state: &AppState) -> Result<credentials::CredentialStatus, String> {
    let key = state
        .cursor_api_key
        .read()
        .map_err(|_| "Не удалось прочитать настройки")?;
    let error = state
        .credential_error
        .read()
        .map_err(|_| "Не удалось прочитать состояние хранилища")?
        .clone();
    Ok(credentials::status(key.as_ref(), error))
}

#[tauri::command]
fn load_app_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    let workspace = state
        .task_store
        .get_setting(WORKSPACE_SETTING)
        .map_err(|error| error.to_string())?
        .or_else(|| {
            state
                .task_store
                .list_tasks()
                .ok()
                .and_then(|tasks| tasks.into_iter().next())
                .map(|task| task.spec.workspace)
        })
        .unwrap_or_default();
    let theme = state
        .task_store
        .get_setting("theme")
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| "system".into());
    let active_project_id = state
        .task_store
        .get_setting("activeProjectId")
        .map_err(|error| error.to_string())?;
    let active_chat_id = state
        .task_store
        .get_setting("activeChatId")
        .map_err(|error| error.to_string())?;
    Ok(AppSettings {
        workspace,
        theme,
        active_project_id,
        active_chat_id,
    })
}

#[tauri::command]
fn save_workspace_setting(workspace: String, state: State<'_, AppState>) -> Result<(), String> {
    let workspace = workspace.trim();
    if workspace.is_empty() {
        return Err("Рабочая папка не задана".into());
    }
    state
        .task_store
        .set_setting(WORKSPACE_SETTING, workspace)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn save_theme_setting(theme: String, state: State<'_, AppState>) -> Result<(), String> {
    if !matches!(theme.as_str(), "system" | "light" | "dark") {
        return Err("Неизвестная тема".into());
    }
    state
        .task_store
        .set_setting("theme", &theme)
        .map_err(|error| error.to_string())
}

fn remember_project(
    state: &AppState,
    project: &yarocursor_core::workspace::Project,
) -> Result<(), String> {
    state
        .task_store
        .set_setting(WORKSPACE_SETTING, &project.path)
        .and_then(|()| {
            state
                .task_store
                .set_setting("activeProjectId", &project.id.to_string())
        })
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn list_projects(
    state: State<'_, AppState>,
) -> Result<Vec<yarocursor_core::workspace::Project>, String> {
    state
        .task_store
        .list_projects()
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn add_existing_project(
    path: String,
    state: State<'_, AppState>,
) -> Result<yarocursor_core::workspace::Project, String> {
    let (path, name) = yarocursor_core::workspace::existing_repository(&path)
        .await
        .map_err(|error| error.to_string())?;
    let project = state
        .task_store
        .upsert_project(&name, &path, None)
        .map_err(|error| error.to_string())?;
    remember_project(&state, &project)?;
    Ok(project)
}

#[tauri::command]
async fn create_repository_project(
    path: String,
    state: State<'_, AppState>,
) -> Result<yarocursor_core::workspace::Project, String> {
    let (path, name) = yarocursor_core::workspace::create_repository(&path)
        .await
        .map_err(|error| error.to_string())?;
    let project = state
        .task_store
        .upsert_project(&name, &path, None)
        .map_err(|error| error.to_string())?;
    remember_project(&state, &project)?;
    Ok(project)
}

#[tauri::command]
async fn clone_repository_project(
    url: String,
    destination: String,
    state: State<'_, AppState>,
) -> Result<yarocursor_core::workspace::Project, String> {
    let (path, name) = yarocursor_core::workspace::clone_repository(&url, &destination)
        .await
        .map_err(|error| error.to_string())?;
    let project = state
        .task_store
        .upsert_project(&name, &path, Some(url.trim()))
        .map_err(|error| error.to_string())?;
    remember_project(&state, &project)?;
    Ok(project)
}

#[tauri::command]
fn select_project(project_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let project_id = Uuid::parse_str(&project_id).map_err(|_| "Некорректный project id")?;
    let project = state
        .task_store
        .get_project(project_id)
        .map_err(|error| error.to_string())?
        .ok_or("Проект не найден")?;
    remember_project(&state, &project)
}

#[tauri::command]
fn create_chat(
    project_id: String,
    title: String,
    state: State<'_, AppState>,
) -> Result<yarocursor_core::workspace::Chat, String> {
    let project_id = Uuid::parse_str(&project_id).map_err(|_| "Некорректный project id")?;
    let chat = state
        .task_store
        .create_chat(project_id, &title)
        .map_err(|error| error.to_string())?;
    state
        .task_store
        .set_setting("activeChatId", &chat.id.to_string())
        .map_err(|error| error.to_string())?;
    Ok(chat)
}

#[tauri::command]
fn list_chats(
    project_id: String,
    archived: bool,
    state: State<'_, AppState>,
) -> Result<Vec<yarocursor_core::workspace::Chat>, String> {
    let project_id = Uuid::parse_str(&project_id).map_err(|_| "Некорректный project id")?;
    state
        .task_store
        .list_chats(project_id, archived)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn archive_chat(chat_id: String, archived: bool, state: State<'_, AppState>) -> Result<(), String> {
    let chat_id = Uuid::parse_str(&chat_id).map_err(|_| "Некорректный chat id")?;
    state
        .task_store
        .archive_chat(chat_id, archived)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn load_chat_session(
    chat_id: String,
    state: State<'_, AppState>,
) -> Result<Option<yarocursor_core::orchestrator::TaskSession>, String> {
    let chat_id = Uuid::parse_str(&chat_id).map_err(|_| "Некорректный chat id")?;
    state
        .task_store
        .set_setting("activeChatId", &chat_id.to_string())
        .map_err(|error| error.to_string())?;
    let Some(task_id) = state
        .task_store
        .task_for_chat(chat_id)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    yarocursor_core::orchestrator::load_session(&state.task_store, task_id)
        .map(Some)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn inspect_environment(
    workspace: String,
    state: State<'_, AppState>,
) -> Result<yarocursor_core::EnvironmentReport, String> {
    let cursor_key = state
        .cursor_api_key
        .read()
        .map_err(|_| "Не удалось прочитать настройки")?
        .as_ref()
        .map(|secret| secret.expose_secret().to_owned());
    let diagnostic_state_root = state.cursor_state_root.join("diagnostics");
    Ok(yarocursor_core::diagnostics::inspect(
        &workspace,
        cursor_key.as_deref(),
        state.cursor_bridge_binary.as_deref(),
        Some(&diagnostic_state_root),
    )
    .await)
}

#[tauri::command]
async fn start_codex_login(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let (client, url) = yarocursor_core::codex::begin_chatgpt_login()
        .await
        .map_err(|error| error.to_string())?;
    let mut active_login = state.codex_login.lock().await;
    if let Some(previous) = active_login.replace(client) {
        previous.shutdown().await;
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| format!("Не удалось открыть браузер: {error}"))
}

#[tauri::command]
async fn create_task_plan(
    request: yarocursor_core::orchestrator::CreateTaskRequest,
    chat_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<yarocursor_core::orchestrator::TaskSession, String> {
    let title = request.goal.trim().chars().take(64).collect::<String>();
    let session = yarocursor_core::orchestrator::create_and_plan(&state.task_store, request)
        .await
        .map_err(|error| error.to_string())?;
    if let Some(chat_id) = chat_id {
        let chat_id = Uuid::parse_str(&chat_id).map_err(|_| "Некорректный chat id")?;
        state
            .task_store
            .link_task_to_chat(chat_id, session.task.id, &title)
            .map_err(|error| error.to_string())?;
    }
    Ok(session)
}

#[tauri::command]
async fn execute_task(
    task_id: String,
    state: State<'_, AppState>,
) -> Result<yarocursor_core::orchestrator::TaskSession, String> {
    let task_id = Uuid::parse_str(&task_id).map_err(|_| "Некорректный task id")?;
    let cursor_api_key = state
        .cursor_api_key
        .read()
        .map_err(|_| "Не удалось прочитать настройки")?
        .as_ref()
        .map(|secret| secret.expose_secret().to_owned());
    {
        let mut active_tasks = state.active_tasks.lock().await;
        if !active_tasks.insert(task_id) {
            return Err("Эта задача уже выполняется".into());
        }
    }
    let result = yarocursor_core::orchestrator::execute_task(
        &state.task_store,
        task_id,
        cursor_api_key.as_deref(),
        state.cursor_bridge_binary.as_deref(),
        &state.worktree_root,
        &state.cursor_state_root,
    )
    .await;
    state.active_tasks.lock().await.remove(&task_id);
    result.map_err(|error| error.to_string())
}

#[tauri::command]
fn open_task_worktree(
    task_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let task_id = Uuid::parse_str(&task_id).map_err(|_| "Некорректный task id")?;
    let worktree = state
        .task_store
        .load_artifact::<PathBuf>(task_id, "git.worktree")
        .map_err(|error| error.to_string())?
        .ok_or("Рабочая папка Grok не найдена")?;
    if !worktree.is_dir() {
        return Err(format!(
            "Рабочая папка не существует: {}",
            worktree.display()
        ));
    }
    app.opener()
        .open_path(worktree.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|error| format!("Не удалось открыть рабочую папку: {error}"))
}

#[tauri::command]
fn latest_task_session(
    state: State<'_, AppState>,
) -> Result<Option<yarocursor_core::orchestrator::TaskSession>, String> {
    let Some(task) = state
        .task_store
        .list_tasks()
        .map_err(|error| error.to_string())?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    yarocursor_core::orchestrator::load_session(&state.task_store, task.id)
        .map(Some)
        .map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let executable = if cfg!(windows) {
                "cursor-sdk-bridge.exe"
            } else {
                "cursor-sdk-bridge"
            };
            let bridge = app
                .path()
                .resource_dir()
                .ok()
                .map(|path| path.join("cursor-sdk-bridge").join("bin").join(executable))
                .filter(|path| path.is_file());
            let app_data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data)?;
            let task_store = TaskStore::open(app_data.join("yarocursor.sqlite"))?;
            yarocursor_core::orchestrator::recover_interrupted_tasks(&task_store)?;
            let worktree_root = app_data.join("worktrees");
            let cursor_state_root = app_data.join("cursor-sdk-state");
            std::fs::create_dir_all(&cursor_state_root)?;
            let (cursor_credential_store, cursor_api_key, credential_error) =
                match credentials::CursorCredentialStore::new() {
                    Ok(store) => match store.load() {
                        Ok(key) => (Some(store), key, None),
                        Err(error) => (Some(store), None, Some(error)),
                    },
                    Err(error) => (None, None, Some(error)),
                };
            app.manage(AppState::new(
                cursor_api_key,
                cursor_credential_store,
                credential_error,
                bridge,
                task_store,
                worktree_root,
                cursor_state_root,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cursor_credential_status,
            set_cursor_api_key,
            delete_cursor_api_key,
            load_app_settings,
            save_workspace_setting,
            save_theme_setting,
            list_projects,
            add_existing_project,
            create_repository_project,
            clone_repository_project,
            select_project,
            create_chat,
            list_chats,
            archive_chat,
            load_chat_session,
            inspect_environment,
            start_codex_login,
            create_task_plan,
            execute_task,
            open_task_worktree,
            latest_task_session
        ])
        .run(tauri::generate_context!())
        .expect("error while running Yarocursor");
}
