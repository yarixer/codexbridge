use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, anyhow};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, broadcast, oneshot},
    time::{Duration, timeout},
};

use crate::diagnostics::{HealthState, ModelCapability, ProviderHealth};
use crate::task::{ArchitectPlan, TaskSpec};

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

pub async fn plan_task(spec: &TaskSpec) -> Result<ArchitectPlan> {
    let prompt = format!(
        "Ты архитектор Yarocursor. Изучи репозиторий только для чтения и составь точный план для одного coding-агента Cursor. Не изменяй файлы и не пиши реализацию. План должен быть достаточно конкретным, чтобы исполнитель мог работать автономно.\n\nЦель:\n{}\n\nОграничения:\n{}\n\nКритерии приёмки:\n{}\n\nКоманды проверки:\n{}",
        spec.goal,
        display_lines(&spec.constraints),
        display_lines(&spec.acceptance_criteria),
        spec.validation_commands
            .iter()
            .map(|command| command.join(" "))
            .collect::<Vec<_>>()
            .join("\n")
    );
    structured_turn(&spec.workspace, &prompt, architect_plan_schema()).await
}

pub async fn review_task(
    workspace: &str,
    spec: &TaskSpec,
    plan: &ArchitectPlan,
    diff: &str,
    validation_summary: &str,
) -> Result<ArchitectReview> {
    let prompt = format!(
        "Ты финальный ревьюер Yarocursor. Проверь реализацию относительно цели, плана и критериев приёмки. Работай только для чтения. Не исправляй код. Одобряй только если нет блокирующих ошибок.\n\nЦель:\n{}\n\nПлан:\n{}\n\nРезультаты проверок:\n{}\n\nGit diff:\n{}",
        spec.goal,
        serde_json::to_string_pretty(plan)?,
        validation_summary,
        truncate(diff, 120_000)
    );
    structured_turn(workspace, &prompt, architect_review_schema()).await
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArchitectReview {
    pub approved: bool,
    pub summary: String,
    #[serde(default)]
    pub issues: Vec<String>,
}

async fn structured_turn<T: DeserializeOwned>(
    workspace: &str,
    prompt: &str,
    output_schema: Value,
) -> Result<T> {
    let client = CodexClient::spawn().await?;
    let result = structured_turn_inner(&client, workspace, prompt, output_schema).await;
    client.shutdown().await;
    result
}

async fn structured_turn_inner<T: DeserializeOwned>(
    client: &CodexClient,
    workspace: &str,
    prompt: &str,
    output_schema: Value,
) -> Result<T> {
    let thread = client
        .request(
            "thread/start",
            json!({
                "model": "gpt-6-astra",
                "cwd": workspace,
                "approvalPolicy": "never",
                "sandbox": "read-only",
                "serviceName": "yarocursor"
            }),
        )
        .await?;
    let thread_id = thread
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .context("Codex не вернул thread.id")?
        .to_owned();
    let mut notifications = client.subscribe();
    let turn = client
        .request(
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": [{ "type": "text", "text": prompt }],
                "cwd": workspace,
                "approvalPolicy": "never",
                "sandboxPolicy": {
                    "type": "readOnly",
                    "access": { "type": "fullAccess" }
                },
                "model": "gpt-6-astra",
                "effort": "high",
                "summary": "concise",
                "outputSchema": output_schema
            }),
        )
        .await?;
    let turn_id = turn
        .pointer("/turn/id")
        .and_then(Value::as_str)
        .context("Codex не вернул turn.id")?
        .to_owned();

    let wait_for_result = async {
        let mut final_text = None;
        loop {
            let notification = notifications
                .recv()
                .await
                .context("канал событий Codex закрыт")?;
            let method = notification.get("method").and_then(Value::as_str);
            if method == Some("item/completed") {
                let item = notification.pointer("/params/item");
                if item
                    .and_then(|value| value.get("type"))
                    .and_then(Value::as_str)
                    == Some("agentMessage")
                {
                    final_text = item
                        .and_then(|value| value.get("text"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                }
            }
            if method != Some("turn/completed") {
                continue;
            }
            let completed_turn = notification.pointer("/params/turn");
            if completed_turn
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                != Some(&turn_id)
            {
                continue;
            }
            let status = completed_turn
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("failed");
            if status != "completed" {
                let message = completed_turn
                    .and_then(|value| value.pointer("/error/message"))
                    .and_then(Value::as_str)
                    .unwrap_or("ход Astra завершился с ошибкой");
                return Err(anyhow!(message.to_owned()));
            }
            let text = final_text.context("Astra не вернула структурированный ответ")?;
            return serde_json::from_str::<T>(strip_json_fence(&text))
                .context("не удалось разобрать структурированный ответ Astra");
        }
    };

    let result = timeout(Duration::from_secs(900), wait_for_result)
        .await
        .context("таймаут хода Astra")?;
    let _ = client
        .request("thread/delete", json!({ "threadId": thread_id }))
        .await;
    result
}

fn architect_plan_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "workItems": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "objective": { "type": "string" },
                        "allowedPaths": { "type": "array", "items": { "type": "string" } },
                        "dependsOn": { "type": "array", "items": { "type": "string" } },
                        "acceptanceCriteria": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["id", "objective", "allowedPaths", "dependsOn", "acceptanceCriteria"],
                    "additionalProperties": false
                }
            },
            "risks": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["summary", "workItems", "risks"],
        "additionalProperties": false
    })
}

fn architect_review_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "approved": { "type": "boolean" },
            "summary": { "type": "string" },
            "issues": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["approved", "summary", "issues"],
        "additionalProperties": false
    })
}

fn display_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        "—".into()
    } else {
        lines.join("\n")
    }
}

fn truncate(value: &str, max_chars: usize) -> &str {
    value
        .char_indices()
        .nth(max_chars)
        .map_or(value, |(index, _)| &value[..index])
}

fn strip_json_fence(value: &str) -> &str {
    let trimmed = value.trim();
    trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|inner| inner.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed)
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
