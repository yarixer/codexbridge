use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, broadcast, oneshot},
    time::{Duration, timeout},
};

use crate::diagnostics::{HealthState, ModelCapability, ProviderHealth};

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

pub struct CodexClient {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    notifications: broadcast::Sender<Value>,
    next_id: AtomicU64,
}

impl CodexClient {
    pub async fn spawn() -> Result<Self> {
        let mut child = Command::new("codex")
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("не удалось запустить `codex app-server`")?;

        let stdin = child.stdin.take().context("app-server не открыл stdin")?;
        let stdout = child.stdout.take().context("app-server не открыл stdout")?;
        let stderr = child.stderr.take().context("app-server не открыл stderr")?;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (notifications, _) = broadcast::channel(512);

        let reader_pending = Arc::clone(&pending);
        let reader_notifications = notifications.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if let Some(id) = message.get("id").and_then(Value::as_u64) {
                    if let Some(sender) = reader_pending.lock().await.remove(&id) {
                        let result = match message.get("error") {
                            Some(error) => Err(error.to_string()),
                            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = sender.send(result);
                    }
                } else {
                    let _ = reader_notifications.send(message);
                }
            }
        });

        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "codex_app_server", %line);
            }
        });

        let client = Self {
            child: Arc::new(Mutex::new(child)),
            stdin: Arc::new(Mutex::new(stdin)),
            pending,
            notifications,
            next_id: AtomicU64::new(1),
        };

        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&self) -> Result<()> {
        self.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "yarocursor",
                    "title": "Yarocursor",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )
        .await?;
        self.notify("initialized", json!({})).await
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        self.write(&json!({ "method": method, "id": id, "params": params }))
            .await?;

        timeout(Duration::from_secs(20), receiver)
            .await
            .context("таймаут ответа Codex App Server")?
            .context("канал ответа Codex App Server закрыт")?
            .map_err(|error| anyhow!("Codex App Server: {error}"))
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.write(&json!({ "method": method, "params": params }))
            .await
    }

    async fn write(&self, message: &Value) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        let mut bytes = serde_json::to_vec(message)?;
        bytes.push(b'\n');
        stdin.write_all(&bytes).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.notifications.subscribe()
    }

    pub async fn shutdown(&self) {
        let _ = self.child.lock().await.kill().await;
    }
}

pub async fn probe() -> ProviderHealth {
    let client = match CodexClient::spawn().await {
        Ok(client) => client,
        Err(error) => {
            return ProviderHealth {
                state: HealthState::MissingDependency,
                title: "Codex CLI не найден".into(),
                detail: error.to_string(),
                models: vec![],
                usage: None,
            };
        }
    };

    let account = client
        .request("account/read", json!({ "refreshToken": false }))
        .await;
    let account_value = match account {
        Ok(value) => value,
        Err(error) => {
            client.shutdown().await;
            return ProviderHealth {
                state: HealthState::Unavailable,
                title: "Codex App Server недоступен".into(),
                detail: error.to_string(),
                models: vec![],
                usage: None,
            };
        }
    };

    let account_type = chatgpt_account_type(&account_value);
    if account_type != Some("chatgpt") {
        client.shutdown().await;
        let detail = match account_type {
            Some("apiKey") => {
                "Codex сейчас использует OpenAI API key. Войди через ChatGPT, чтобы расходовать лимиты подписки."
            }
            Some(_) => {
                "Codex использует другой способ авторизации. Войди через ChatGPT, чтобы Astra работала по подписке."
            }
            None => "Войди через ChatGPT, чтобы Astra использовала лимиты подписки.",
        };
        return ProviderHealth {
            state: HealthState::NeedsLogin,
            title: "Требуется вход в ChatGPT".into(),
            detail: detail.into(),
            models: vec![],
            usage: None,
        };
    }

    let models_value = client
        .request(
            "model/list",
            json!({ "limit": 100, "includeHidden": false }),
        )
        .await
        .ok();
    let usage = client
        .request("account/rateLimits/read", json!({}))
        .await
        .ok();
    let models = models_value.as_ref()
        .and_then(|value| value.get("data"))
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(|item| {
            let id = item.get("id")?.as_str()?.to_owned();
            Some(ModelCapability {
                display_name: item.get("displayName").and_then(Value::as_str).unwrap_or(&id).to_owned(),
                parameters: json!({
                    "reasoningEfforts": item.get("supportedReasoningEfforts").cloned().unwrap_or(json!([])),
                    "defaultReasoningEffort": item.get("defaultReasoningEffort").cloned().unwrap_or(Value::Null)
                }),
                id,
            })
        }).collect::<Vec<_>>())
        .unwrap_or_default();

    let astra = models.iter().any(|model| model.id == "gpt-6-astra");
    client.shutdown().await;
    ProviderHealth {
        state: if astra {
            HealthState::Ready
        } else {
            HealthState::Unavailable
        },
        title: if astra {
            "Astra готова"
        } else {
            "Astra не найдена"
        }
        .into(),
        detail: if astra {
            "Codex подключён через ChatGPT; модель gpt-6-astra доступна.".into()
        } else {
            format!(
                "Codex авторизован, но gpt-6-astra отсутствует в каталоге (моделей: {}).",
                models.len()
            )
        },
        models,
        usage,
    }
}

pub async fn begin_chatgpt_login() -> Result<(CodexClient, String)> {
    let client = CodexClient::spawn().await?;
    let result = client
        .request(
            "account/login/start",
            json!({
                "type": "chatgpt",
                "useHostedLoginSuccessPage": true,
                "appBrand": "chatgpt"
            }),
        )
        .await?;
    let url = result
        .get("authUrl")
        .and_then(Value::as_str)
        .context("Codex не вернул authUrl")?
        .to_owned();
    Ok((client, url))
}

fn chatgpt_account_type(account_read: &Value) -> Option<&str> {
    account_read
        .get("account")
        .and_then(|account| account.get("type"))
        .and_then(Value::as_str)
        .or_else(|| account_read.get("authMode").and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::chatgpt_account_type;
    use serde_json::json;

    #[test]
    fn reads_chatgpt_account_from_current_app_server_shape() {
        let response = json!({
            "account": {
                "type": "chatgpt",
                "email": "developer@example.com",
                "planType": "plus"
            },
            "requiresOpenaiAuth": true
        });

        assert_eq!(chatgpt_account_type(&response), Some("chatgpt"));
    }

    #[test]
    fn treats_null_account_as_signed_out() {
        let response = json!({ "account": null, "requiresOpenaiAuth": true });

        assert_eq!(chatgpt_account_type(&response), None);
    }
}
