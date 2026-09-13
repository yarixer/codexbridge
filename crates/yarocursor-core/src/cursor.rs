use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
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
    async fn spawn(binary: &Path, workspace: &str, api_key: &str) -> Result<Self> {
        let mut child = Command::new(binary)
            .args(["--workspace", workspace])
            .env("CURSOR_API_KEY", api_key)
            .env("CURSOR_SDK_CLIENT_LANGUAGE", "rust")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("не удалось запустить {}", binary.display()))?;
        let stderr = child.stderr.take().context("bridge не открыл stderr")?;
        let (ready_sender, ready_receiver) = oneshot::channel();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut ready_sender = Some(ready_sender);
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(payload) = line.strip_prefix(READY_PREFIX) {
                    if let Some(sender) = ready_sender.take() {
                        let discovery = serde_json::from_str::<Discovery>(payload)
                            .context("неверная ready-строка bridge");
                        let _ = sender.send(discovery);
                    }
                } else {
                    tracing::debug!(target: "cursor_sdk_bridge", %line);
                }
            }
            if let Some(sender) = ready_sender {
                let _ = sender.send(Err(anyhow::anyhow!("bridge завершился до ready-строки")));
            }
        });
        let discovery = timeout(Duration::from_secs(30), ready_receiver)
            .await
            .context("таймаут запуска Cursor SDK Bridge")?
            .context("канал запуска Cursor SDK Bridge закрыт")??;

        if discovery.schema_version != 1
            || discovery.transport != "tcp"
            || discovery.protocol != "connect"
        {
            bail!("неподдерживаемый протокол Cursor SDK Bridge")
        }
        let token = std::fs::read_to_string(&discovery.auth_token_file)
            .context("не удалось прочитать bridge bearer token")?;
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
            .context("bridge вернул не-JSON ответ")?;
        if !status.is_success() {
            bail!("Cursor Bridge {}: {}", status, body)
        }
        Ok(body)
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
) -> ProviderHealth {
    let Some(binary) = bridge_binary(preferred_binary) else {
        return ProviderHealth {
            state: HealthState::MissingDependency,
            title: "Cursor SDK Bridge не установлен".into(),
            detail: "Установи закреплённую версию bridge или укажи CURSOR_SDK_BRIDGE_BIN.".into(),
            models: vec![],
            usage: None,
        };
    };
    let Some(api_key) = api_key.filter(|key| !key.trim().is_empty()) else {
        let detail = match Bridge::spawn(&binary, workspace, "").await {
            Ok(bridge) => {
                let version = bridge
                    .unary("SdkBridgeControlService", "GetVersion", json!({}))
                    .await;
                bridge.shutdown().await;
                match version {
                    Ok(version) => format!(
                        "Bridge отвечает (версия {}). Добавь Cursor API key для проверки каталога моделей.",
                        version
                            .get("bridgeVersion")
                            .or_else(|| version.get("serverVersion"))
                            .unwrap_or(&version)
                    ),
                    Err(error) => {
                        format!("Bridge найден, но проверка протокола не прошла: {error}")
                    }
                }
            }
            Err(error) => format!("Bridge найден, но не запустился: {error}"),
        };
        return ProviderHealth {
            state: HealthState::NeedsApiKey,
            title: "Нужен Cursor API key".into(),
            detail,
            models: vec![],
            usage: None,
        };
    };

    let bridge = match Bridge::spawn(&binary, workspace, api_key).await {
        Ok(bridge) => bridge,
        Err(error) => {
            return ProviderHealth {
                state: HealthState::Unavailable,
                title: "Cursor Bridge не запустился".into(),
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
                    "Grok 4.6 xHigh Fast готов"
                } else if grok.is_some() {
                    "Нет конфигурации xHigh Fast"
                } else {
                    "Grok 4.6 не найден"
                }
                .into(),
                detail: format!(
                    "Bridge отвечает; версия: {}.",
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
            title: "Ошибка Cursor SDK".into(),
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
}
