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
            .context("failed to start `codex app-server`")?;

        let stdin = child
            .stdin
            .take()
            .context("app-server did not open stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("app-server did not open stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("app-server did not open stderr")?;
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
                    "name": "codexbridge",
                    "title": "CodexBridge",
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
            .context("Codex App Server response timed out")?
            .context("Codex App Server response channel closed")?
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
                title: "Codex CLI not found".into(),
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
                title: "Codex App Server unavailable".into(),
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
                "Codex is currently using an OpenAI API key. Sign in with ChatGPT to use your subscription limits."
            }
            Some(_) => {
                "Codex is using a different authentication method. Sign in with ChatGPT so Astra can use your subscription."
            }
            None => "Sign in with ChatGPT so Astra can use your subscription limits.",
        };
        return ProviderHealth {
            state: HealthState::NeedsLogin,
            title: "ChatGPT sign-in required".into(),
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
            "Astra is ready"
        } else {
            "Astra not found"
        }
        .into(),
        detail: if astra {
            "Codex is connected through ChatGPT; the gpt-6-astra model is available.".into()
        } else {
            format!(
                "Codex is authenticated, but gpt-6-astra is missing from the catalog (models: {}).",
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
        .context("Codex did not return authUrl")?
        .to_owned();
    Ok((client, url))
}

pub async fn plan_task(spec: &TaskSpec) -> Result<ArchitectPlan> {
    let prompt = format!(
        "You are the CodexBridge architect. Inspect the repository in read-only mode and create a precise plan for a single Cursor coding agent. Do not modify files or write the implementation. The plan must be specific enough for the implementation agent to work autonomously.\n\nGoal:\n{}\n\nConstraints:\n{}\n\nAcceptance criteria:\n{}\n\nValidation commands:\n{}",
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
        "You are the final CodexBridge reviewer. Review the implementation against the goal, plan, and acceptance criteria. Work in read-only mode. Do not fix the code. Approve only if there are no blocking issues.\n\nGoal:\n{}\n\nPlan:\n{}\n\nValidation results:\n{}\n\nGit diff:\n{}",
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
                "serviceName": "codexbridge"
            }),
        )
        .await?;
    let thread_id = thread
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .context("Codex did not return thread.id")?
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
        .context("Codex did not return turn.id")?
        .to_owned();

    let wait_for_result = async {
        let mut final_text = None;
        loop {
            let notification = notifications
                .recv()
                .await
                .context("Codex event channel closed")?;
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
                    .unwrap_or("Astra turn failed");
                return Err(anyhow!(message.to_owned()));
            }
            let text = final_text.context("Astra did not return a structured response")?;
            return serde_json::from_str::<T>(strip_json_fence(&text))
                .context("failed to parse Astra's structured response");
        }
    };

    let result = timeout(Duration::from_secs(900), wait_for_result)
        .await
        .context("Astra turn timed out")?;
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
