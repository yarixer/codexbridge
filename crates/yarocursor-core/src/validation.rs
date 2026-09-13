use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    process::Command,
    time::{Duration, timeout},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ValidationResult {
    pub command: Vec<String>,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub output: String,
}

pub async fn run_all(worktree: &Path, commands: &[Vec<String>]) -> Result<Vec<ValidationResult>> {
    let mut results = Vec::with_capacity(commands.len());
    for command in commands {
        let Some(program) = command.first() else {
            continue;
        };
        let child = Command::new(program)
            .args(&command[1..])
            .current_dir(worktree)
            .output();
        let output = timeout(Duration::from_secs(900), child)
            .await
            .with_context(|| format!("таймаут команды `{}`", command.join(" ")))??;
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        results.push(ValidationResult {
            command: command.clone(),
            success: output.status.success(),
            exit_code: output.status.code(),
            output: truncate_owned(combined, 24_000),
        });
    }
    Ok(results)
}

fn truncate_owned(value: String, max_chars: usize) -> String {
    value
        .char_indices()
        .nth(max_chars)
        .map_or(value.clone(), |(index, _)| value[..index].to_owned())
}
