use std::{collections::HashSet, path::PathBuf, sync::RwLock};

use secrecy::{ExposeSecret, SecretString};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;
use yarocursor_core::store::TaskStore;

struct AppState {
    cursor_api_key: RwLock<Option<SecretString>>,
    codex_login: tokio::sync::Mutex<Option<yarocursor_core::codex::CodexClient>>,
    cursor_bridge_binary: Option<PathBuf>,
    task_store: TaskStore,
    worktree_root: PathBuf,
    cursor_state_root: PathBuf,
    active_tasks: tokio::sync::Mutex<HashSet<Uuid>>,
}

impl AppState {
    fn new(
        cursor_bridge_binary: Option<PathBuf>,
        task_store: TaskStore,
        worktree_root: PathBuf,
        cursor_state_root: PathBuf,
    ) -> Self {
        Self {
            cursor_api_key: RwLock::new(None),
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
fn set_cursor_api_key(value: String, state: State<'_, AppState>) -> Result<(), String> {
    let value = value.trim().to_owned();
    let mut key = state
        .cursor_api_key
        .write()
        .map_err(|_| "Не удалось заблокировать настройки")?;
    *key = (!value.is_empty()).then(|| SecretString::from(value));
    Ok(())
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
    state: State<'_, AppState>,
) -> Result<yarocursor_core::orchestrator::TaskSession, String> {
    yarocursor_core::orchestrator::create_and_plan(&state.task_store, request)
        .await
        .map_err(|error| error.to_string())
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
        .map(|secret| secret.expose_secret().to_owned())
        .ok_or("Cursor API key не задан")?;
    {
        let mut active_tasks = state.active_tasks.lock().await;
        if !active_tasks.insert(task_id) {
            return Err("Эта задача уже выполняется".into());
        }
    }
    let result = yarocursor_core::orchestrator::execute_task(
        &state.task_store,
        task_id,
        &cursor_api_key,
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
            app.manage(AppState::new(
                bridge,
                task_store,
                worktree_root,
                cursor_state_root,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_cursor_api_key,
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
