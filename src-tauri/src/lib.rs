use std::{path::PathBuf, sync::RwLock};

use secrecy::{ExposeSecret, SecretString};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

struct AppState {
    cursor_api_key: RwLock<Option<SecretString>>,
    codex_login: tokio::sync::Mutex<Option<yarocursor_core::codex::CodexClient>>,
    cursor_bridge_binary: Option<PathBuf>,
}

impl AppState {
    fn new(cursor_bridge_binary: Option<PathBuf>) -> Self {
        Self {
            cursor_api_key: RwLock::new(None),
            codex_login: tokio::sync::Mutex::new(None),
            cursor_bridge_binary,
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
    Ok(yarocursor_core::diagnostics::inspect(
        &workspace,
        cursor_key.as_deref(),
        state.cursor_bridge_binary.as_deref(),
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
            app.manage(AppState::new(bridge));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_cursor_api_key,
            inspect_environment,
            start_codex_login
        ])
        .run(tauri::generate_context!())
        .expect("error while running Yarocursor");
}
