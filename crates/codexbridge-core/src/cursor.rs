use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::oneshot,
    time::{Duration, timeout},
};

use crate::diagnostics::{HealthState, ModelCapability, ProviderHealth};

const READY_PREFIX: &str = "cursor-sdk-bridge ready ";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Discovery {
    schema_version: u32,
    transport: String,
    protocol: String,
    url: String,
    auth_token_file: PathBuf,
}

struct Bridge {
    child: Child,
    base_url: String,
    token: String,
}

impl Bridge {
    async fn spawn(
        binary: &Path,
        workspace: &str,
        api_key: &str,
        state_root: Option<&Path>,
    ) -> Result<Self> {
        let mut command = Command::new(binary);
        command
            .args(["--workspace", workspace])
            .env("CURSOR_API_KEY", api_key)
            .env("CURSOR_SDK_CLIENT_LANGUAGE", "rust")
            .current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(state_root) = state_root {
            std::fs::create_dir_all(state_root).with_context(|| {
                format!(
                    "failed to create Cursor state directory {}",
                    state_root.display()
                )
            })?;
            command.arg("--state-root").arg(state_root);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to start {}", binary.display()))?;
        let stderr = child.stderr.take().context("bridge did not open stderr")?;
        let (ready_sender, ready_receiver) = oneshot::channel();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut ready_sender = Some(ready_sender);
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(payload) = line.strip_prefix(READY_PREFIX) {
                    if let Some(sender) = ready_sender.take() {
                        let discovery = serde_json::from_str::<Discovery>(payload)
                            .context("invalid bridge ready line");
                        let _ = sender.send(discovery);
                    }
                } else {
                    tracing::debug!(target: "cursor_sdk_bridge", %line);
                }
            }
            if let Some(sender) = ready_sender {
                let _ = sender.send(Err(anyhow::anyhow!("bridge exited before its ready line")));
            }
        });
        let discovery = timeout(Duration::from_secs(30), ready_receiver)
            .await
            .context("Cursor SDK Bridge startup timed out")?
            .context("Cursor SDK Bridge startup channel closed")??;

        if discovery.schema_version != 1
            || discovery.transport != "tcp"
            || discovery.protocol != "connect"
        {
            bail!("unsupported Cursor SDK Bridge protocol")
        }
        let token = std::fs::read_to_string(&discovery.auth_token_file)
            .context("failed to read the bridge bearer token")?;
        Ok(Self {
            child,
            base_url: discovery.url,
            token: token.trim().to_owned(),
        })
    }

    async fn unary(&self, service: &str, method: &str, body: Value) -> Result<Value> {
        let response = reqwest::Client::new()
            .post(format!("{}/sdk.v1.{service}/{method}", self.base_url))
            .bearer_auth(&self.token)
            .header("Connect-Protocol-Version", "1")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .context("bridge returned a non-JSON response")?;
        if !status.is_success() {
            bail!("Cursor Bridge {}: {}", status, body)
        }
        Ok(body)
    }

    async fn server_stream(&self, service: &str, method: &str, body: Value) -> Result<Vec<Value>> {
        let payload = serde_json::to_vec(&body)?;
        let length = u32::try_from(payload.len()).context("Cursor request is too large")?;
        let mut framed = Vec::with_capacity(payload.len() + 5);
        framed.push(0);
        framed.extend_from_slice(&length.to_be_bytes());
        framed.extend_from_slice(&payload);
        let response = reqwest::Client::new()
            .post(format!("{}/sdk.v1.{service}/{method}", self.base_url))
            .bearer_auth(&self.token)
            .header("Connect-Protocol-Version", "1")
            .header("Content-Type", "application/connect+json")
            .body(framed)
            .send()
            .await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            bail!(
                "Cursor Bridge {}: {}",
                status,
                String::from_utf8_lossy(&bytes)
            );
        }
        decode_connect_json_stream(&bytes)
    }

    async fn shutdown(mut self) {
        let _ = self
            .unary("SdkBridgeControlService", "Shutdown", json!({}))
            .await;
        if timeout(Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CursorRunResult {
    pub agent_id: String,
    pub run_id: String,
    pub status: String,
    pub text: String,
    pub duration_ms: u64,
    pub events: Vec<CursorRunEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CursorRunEvent {
    pub kind: String,
    pub summary: String,
}

pub async fn execute(
    workspace: &Path,
    api_key: &str,
    preferred_binary: Option<&Path>,
    state_root: &Path,
    prompt: &str,
) -> Result<CursorRunResult> {
    let binary = bridge_binary(preferred_binary)
        .context("Cursor SDK Bridge is not installed or could not be found")?;
    let workspace_text = workspace
        .to_str()
        .context("worktree path contains unsupported characters")?;
    std::fs::create_dir_all(state_root).with_context(|| {
        format!(
            "failed to create Cursor state directory {}",
            state_root.display()
        )
    })?;
    let state_root_text = state_root
        .to_str()
        .context("Cursor state path contains unsupported characters")?;
    let bridge = Bridge::spawn(&binary, workspace_text, api_key, Some(state_root)).await?;
    let result = execute_inner(&bridge, workspace_text, api_key, state_root_text, prompt).await;
    bridge.shutdown().await;
    result
}

async fn execute_inner(
    bridge: &Bridge,
    workspace: &str,
    api_key: &str,
    state_root: &str,
    prompt: &str,
) -> Result<CursorRunResult> {
    let catalog = bridge
        .unary(
            "SdkCursorService",
            "ListModels",
            json!({ "options": { "apiKey": api_key } }),
        )
        .await?;
    let model = grok_model_selection(&catalog)?;
    let created = bridge
        .unary(
            "SdkAgentService",
            "CreateAgent",
            json!({
                "options": {
                    "model": model,
                    "apiKey": api_key,
                    "name": "CodexBridge worker",
                    "local": {
                        "cwd": [workspace],
                        "sandboxOptions": { "enabled": false },
                        "store": { "type": "jsonl", "rootDir": state_root },
                        "autoReview": false
                    }
                }
            }),
        )
        .await?;
    let agent_id = created
        .get("agentId")
        .and_then(Value::as_str)
        .context("Cursor did not return agentId")?
        .to_owned();
    let streamed = bridge
        .server_stream(
            "SdkAgentService",
            "Send",
            json!({
                "agentId": agent_id,
                "message": { "text": prompt },
                "options": { "enableDeltas": false, "enableSteps": false }
            }),
        )
        .await;
    let _ = bridge
        .unary(
            "SdkAgentService",
            "CloseAgent",
            json!({ "agentId": agent_id }),
        )
        .await;
    parse_cursor_run(&agent_id, streamed?)
}

fn grok_model_selection(catalog: &Value) -> Result<Value> {
    let model = catalog
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some("grok-4.6"))
        })
        .context("grok-4.6 is missing from the Cursor catalog")?;
    let mut params = model
        .get("variants")
        .and_then(Value::as_array)
        .and_then(|variants| {
            variants.iter().find(|variant| {
                variant
                    .get("displayName")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.to_ascii_lowercase().contains("fast"))
            })
        })
        .and_then(|variant| variant.get("params"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let definitions = model
        .get("parameters")
        .and_then(Value::as_array)
        .context("grok-4.6 has no model parameters")?;
    let effort_id = definitions
        .iter()
        .find(|parameter| has_parameter_value(parameter, "xhigh"))
        .and_then(|parameter| parameter.get("id"))
        .and_then(Value::as_str)
        .context("grok-4.6 does not support xhigh")?;
    upsert_model_parameter(&mut params, effort_id, "xhigh");

    if let Some(fast_id) = definitions
        .iter()
        .find(|parameter| {
            has_parameter_value(parameter, "true") && parameter_id_contains(parameter, "fast")
        })
        .and_then(|parameter| parameter.get("id"))
        .and_then(Value::as_str)
    {
        upsert_model_parameter(&mut params, fast_id, "true");
    }
    let has_fast = params.iter().any(|parameter| {
        parameter
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.to_ascii_lowercase().contains("fast"))
            && parameter.get("value").and_then(Value::as_str) == Some("true")
    }) || model
        .get("variants")
        .and_then(Value::as_array)
        .is_some_and(|variants| {
            variants.iter().any(|variant| {
                variant
                    .get("displayName")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.to_ascii_lowercase().contains("fast"))
                    && variant.get("params").and_then(Value::as_array).is_some_and(
                        |variant_params| variant_params.iter().all(|value| params.contains(value)),
                    )
            })
        });
    if !has_fast {
        bail!("grok-4.6 has no Fast configuration")
    }
    Ok(json!({ "id": "grok-4.6", "params": params }))
}

fn has_parameter_value(parameter: &Value, expected: &str) -> bool {
    parameter
        .get("values")
        .and_then(Value::as_array)
        .is_some_and(|values| {
            values
                .iter()
                .any(|value| value.get("value").and_then(Value::as_str) == Some(expected))
        })
}

fn parameter_id_contains(parameter: &Value, expected: &str) -> bool {
    parameter
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| id.to_ascii_lowercase().contains(expected))
}

fn upsert_model_parameter(params: &mut Vec<Value>, id: &str, value: &str) {
    params.retain(|parameter| parameter.get("id").and_then(Value::as_str) != Some(id));
    params.push(json!({ "id": id, "value": value }));
}

fn decode_connect_json_stream(bytes: &[u8]) -> Result<Vec<Value>> {
    let mut cursor = 0usize;
    let mut messages = Vec::new();
    let mut saw_end = false;
    while cursor < bytes.len() {
        if bytes.len() - cursor < 5 {
            bail!("Cursor stream contains an incomplete frame header")
        }
        let flags = bytes[cursor];
        let length = u32::from_be_bytes([
            bytes[cursor + 1],
            bytes[cursor + 2],
            bytes[cursor + 3],
            bytes[cursor + 4],
        ]) as usize;
        cursor += 5;
        let end = cursor
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .context("Cursor stream contains an incomplete frame")?;
        let payload = &bytes[cursor..end];
        cursor = end;
        if flags & 0x01 != 0 {
            bail!("compressed Cursor stream frames are not supported yet")
        }
        let value = if payload.is_empty() {
            json!({})
        } else {
            serde_json::from_slice::<Value>(payload).context("invalid JSON in Cursor stream")?
        };
        if flags & 0x02 != 0 {
            saw_end = true;
            if let Some(error) = value.get("error") {
                bail!("Cursor stream: {error}")
            }
            break;
        }
        messages.push(value);
    }
    if !saw_end {
        bail!("Cursor stream ended without EndStreamResponse")
    }
    Ok(messages)
}

fn parse_cursor_run(agent_id: &str, messages: Vec<Value>) -> Result<CursorRunResult> {
    let mut events = Vec::new();
    let mut last_status_message = None;
    let mut terminal = None;
    for message in &messages {
        if let Some(sdk_message) = message.get("sdkMessage") {
            let kind = sdk_message
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned();
            let payload = sdk_message.get("message").cloned().unwrap_or(Value::Null);
            let summary = summarize_cursor_event(&kind, &payload);
            if kind == "status"
                && let Some(value) = payload.get("message").and_then(Value::as_str)
            {
                last_status_message = Some(value.to_owned());
            }
            events.push(CursorRunEvent { kind, summary });
        }
        if let Some(result) = message.get("result") {
            terminal = Some(result);
        }
    }
    let terminal = terminal.context("Cursor stream did not return a terminal result")?;
    let result = terminal.get("result").unwrap_or(terminal);
    let status = result
        .get("status")
        .or_else(|| terminal.get("status"))
        .map(value_to_string)
        .unwrap_or_else(|| "unknown".into());
    if !status.to_ascii_lowercase().contains("finished") && status != "3" {
        return Err(anyhow!(
            "Grok run ended with status {status}: {}",
            last_status_message.unwrap_or_else(|| "no error description".into())
        ));
    }
    Ok(CursorRunResult {
        agent_id: terminal
            .get("agentId")
            .or_else(|| result.get("agentId"))
            .and_then(Value::as_str)
            .unwrap_or(agent_id)
            .to_owned(),
        run_id: terminal
            .get("runId")
            .or_else(|| result.get("runId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        status,
        text: result
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        duration_ms: result
            .get("durationMs")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        events,
    })
}

fn value_to_string(value: &Value) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn summarize_cursor_event(kind: &str, payload: &Value) -> String {
    match kind {
        "tool_call" => payload
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_owned(),
        "status" => payload
            .get("message")
            .or_else(|| payload.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("status")
            .to_owned(),
        "assistant" => payload
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default(),
        _ => kind.to_owned(),
    }
}

fn bridge_binary(preferred: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = preferred.filter(|path| path.is_file()) {
        return Some(path.to_owned());
    }
    if let Some(path) = env::var_os("CURSOR_SDK_BRIDGE_BIN").map(PathBuf::from)
        && path.is_file()
    {
        return Some(path);
    }
    let executable = if cfg!(windows) {
        "cursor-sdk-bridge.exe"
    } else {
        "cursor-sdk-bridge"
    };
    env::var_os("PATH")
        .and_then(|paths| {
            env::split_paths(&paths)
                .map(|path| path.join(executable))
                .find(|path| path.is_file())
        })
        .or_else(|| {
            let local = env::current_dir()
                .ok()?
                .join(".tools")
                .join("cursor-sdk-bridge")
                .join("bin")
                .join(executable);
            local.is_file().then_some(local)
        })
}

pub async fn probe(
    workspace: &str,
    api_key: Option<&str>,
    preferred_binary: Option<&Path>,
    state_root: Option<&Path>,
) -> ProviderHealth {
    let Some(binary) = bridge_binary(preferred_binary) else {
        return ProviderHealth {
            state: HealthState::MissingDependency,
            title: "Cursor SDK Bridge is not installed".into(),
            detail: "Install the pinned bridge version or set CURSOR_SDK_BRIDGE_BIN.".into(),
            models: vec![],
            usage: None,
        };
    };
    let Some(api_key) = api_key.filter(|key| !key.trim().is_empty()) else {
        let detail = match Bridge::spawn(&binary, workspace, "", state_root).await {
            Ok(bridge) => {
                let version = bridge
                    .unary("SdkBridgeControlService", "GetVersion", json!({}))
                    .await;
                bridge.shutdown().await;
                match version {
                    Ok(version) => format!(
                        "Bridge is responding (version {}). Add a Cursor API key to check the model catalog.",
                        version
                            .get("bridgeVersion")
                            .or_else(|| version.get("serverVersion"))
                            .unwrap_or(&version)
                    ),
                    Err(error) => {
                        format!("Bridge was found, but the protocol check failed: {error}")
                    }
                }
            }
            Err(error) => format!("Bridge was found but failed to start: {error}"),
        };
        return ProviderHealth {
            state: HealthState::NeedsApiKey,
            title: "Cursor API key required".into(),
            detail,
            models: vec![],
            usage: None,
        };
    };

    let bridge = match Bridge::spawn(&binary, workspace, api_key, state_root).await {
        Ok(bridge) => bridge,
        Err(error) => {
            return ProviderHealth {
                state: HealthState::Unavailable,
                title: "Cursor Bridge failed to start".into(),
                detail: error.to_string(),
                models: vec![],
                usage: None,
            };
        }
    };
    let result = async {
        bridge
            .unary("SdkBridgeControlService", "Ping", json!({}))
            .await?;
        let version = bridge
            .unary("SdkBridgeControlService", "GetVersion", json!({}))
            .await?;
        let catalog = bridge
            .unary(
                "SdkCursorService",
                "ListModels",
                json!({ "options": { "apiKey": api_key } }),
            )
            .await?;
        Ok::<_, anyhow::Error>((version, catalog))
    }
    .await;

    let health = match result {
        Ok((version, catalog)) => {
            let items = catalog
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let models = items
                .iter()
                .filter_map(|item| {
                    let id = item.get("id")?.as_str()?.to_owned();
                    Some(ModelCapability {
                        display_name: item
                            .get("displayName")
                            .and_then(Value::as_str)
                            .unwrap_or(&id)
                            .to_owned(),
                        parameters: json!({
                            "parameters": item.get("parameters").cloned().unwrap_or(json!([])),
                            "variants": item.get("variants").cloned().unwrap_or(json!([]))
                        }),
                        id,
                    })
                })
                .collect::<Vec<_>>();
            let grok = models.iter().find(|model| model.id == "grok-4.6");
            let supports_xhigh_fast = grok.is_some_and(has_xhigh_fast);
            ProviderHealth {
                state: if supports_xhigh_fast {
                    HealthState::Ready
                } else {
                    HealthState::Unavailable
                },
                title: if supports_xhigh_fast {
                    "Grok 4.6 xHigh Fast is ready"
                } else if grok.is_some() {
                    "No xHigh Fast configuration"
                } else {
                    "Grok 4.6 not found"
                }
                .into(),
                detail: format!(
                    "Bridge is responding; version: {}.",
                    version
                        .get("bridgeVersion")
                        .or_else(|| version.get("serverVersion"))
                        .unwrap_or(&version)
                ),
                models,
                usage: None,
            }
        }
        Err(error) => ProviderHealth {
            state: HealthState::Unavailable,
            title: "Cursor SDK error".into(),
            detail: error.to_string(),
            models: vec![],
            usage: None,
        },
    };
    bridge.shutdown().await;
    health
}

fn has_xhigh_fast(model: &ModelCapability) -> bool {
    let parameters = model.parameters.get("parameters").and_then(Value::as_array);
    let variants = model.parameters.get("variants").and_then(Value::as_array);

    let has_xhigh = parameters.is_some_and(|items| {
        items.iter().any(|parameter| {
            parameter
                .get("values")
                .and_then(Value::as_array)
                .is_some_and(|values| {
                    values
                        .iter()
                        .any(|value| value.get("value").and_then(Value::as_str) == Some("xhigh"))
                })
        })
    });
    let has_fast_parameter = parameters.is_some_and(|items| {
        items.iter().any(|parameter| {
            parameter.get("id").and_then(Value::as_str) == Some("fast")
                && parameter
                    .get("values")
                    .and_then(Value::as_array)
                    .is_some_and(|values| {
                        values
                            .iter()
                            .any(|value| value.get("value").and_then(Value::as_str) == Some("true"))
                    })
        })
    });
    let fast_variant = variants.is_some_and(|items| {
        items.iter().any(|variant| {
            variant
                .get("displayName")
                .and_then(Value::as_str)
                .is_some_and(|name| name.to_ascii_lowercase().contains("fast"))
        })
    });

    has_xhigh && (has_fast_parameter || fast_variant)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_required_grok_configuration() {
        let model = ModelCapability {
            id: "grok-4.6".into(),
            display_name: "Grok 4.6".into(),
            parameters: json!({
                "parameters": [
                    { "id": "effort", "values": [{ "value": "low" }, { "value": "xhigh" }] },
                    { "id": "fast", "values": [{ "value": "false" }, { "value": "true" }] }
                ],
                "variants": []
            }),
        };

        assert!(has_xhigh_fast(&model));
    }

    #[test]
    fn rejects_model_without_xhigh() {
        let model = ModelCapability {
            id: "grok-4.6".into(),
            display_name: "Grok 4.6".into(),
            parameters: json!({
                "parameters": [{ "id": "effort", "values": [{ "value": "high" }] }],
                "variants": [{ "displayName": "Fast" }]
            }),
        };

        assert!(!has_xhigh_fast(&model));
    }

    #[test]
    fn decodes_connect_json_frames() {
        fn frame(flags: u8, value: Value) -> Vec<u8> {
            let payload = serde_json::to_vec(&value).unwrap();
            let mut result = vec![flags];
            result.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            result.extend(payload);
            result
        }
        let mut stream = frame(0, json!({ "sdkMessage": { "type": "system" } }));
        stream.extend(frame(2, json!({})));

        let decoded = decode_connect_json_stream(&stream).unwrap();

        assert_eq!(decoded.len(), 1);
        assert_eq!(
            decoded[0]
                .pointer("/sdkMessage/type")
                .and_then(Value::as_str),
            Some("system")
        );
    }

    #[test]
    fn selects_xhigh_fast_variant() {
        let catalog = json!({
            "items": [{
                "id": "grok-4.6",
                "parameters": [
                    { "id": "effort", "values": [{ "value": "low" }, { "value": "xhigh" }] }
                ],
                "variants": [{
                    "displayName": "Fast",
                    "params": [{ "id": "fast", "value": "true" }]
                }]
            }]
        });

        let selection = grok_model_selection(&catalog).unwrap();

        assert_eq!(
            selection.get("id").and_then(Value::as_str),
            Some("grok-4.6")
        );
        assert!(
            selection["params"]
                .as_array()
                .unwrap()
                .contains(&json!({ "id": "effort", "value": "xhigh" }))
        );
        assert!(
            selection["params"]
                .as_array()
                .unwrap()
                .contains(&json!({ "id": "fast", "value": "true" }))
        );
    }
}
