use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::{codex, cursor};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealth {
    pub state: HealthState,
    pub title: String,
    pub detail: String,
    #[serde(default)]
    pub models: Vec<ModelCapability>,
    pub usage: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HealthState {
    Ready,
    NeedsLogin,
    NeedsApiKey,
    MissingDependency,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapability {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentReport {
    pub codex: ProviderHealth,
    pub cursor: ProviderHealth,
    pub workspace: String,
}

pub async fn inspect(
    workspace: &str,
    cursor_api_key: Option<&str>,
    cursor_bridge_binary: Option<&Path>,
    cursor_state_root: Option<&Path>,
) -> EnvironmentReport {
    let (codex, cursor) = tokio::join!(
        codex::probe(),
        cursor::probe(
            workspace,
            cursor_api_key,
            cursor_bridge_binary,
            cursor_state_root,
        ),
    );

    EnvironmentReport {
        codex,
        cursor,
        workspace: workspace.to_owned(),
    }
}
