use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
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
        let executable = resolve_program(program).ok_or_else(|| {
            anyhow!(
                "программа `{program}` не найдена в PATH для команды `{}`",
                command.join(" ")
            )
        })?;
        let child = Command::new(&executable)
            .args(&command[1..])
            .current_dir(worktree)
            .output();
        let output = timeout(Duration::from_secs(900), child)
            .await
            .with_context(|| format!("таймаут команды `{}`", command.join(" ")))?
            .with_context(|| {
                format!(
                    "не удалось запустить команду `{}` через {}",
                    command.join(" "),
                    executable.display()
                )
            })?;
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

fn resolve_program(program: &str) -> Option<PathBuf> {
    let requested = Path::new(program);
    if requested.components().count() > 1 || requested.is_absolute() {
        return Some(requested.to_owned());
    }

    let directories =
        env::var_os("PATH").map(|value| env::split_paths(&value).collect::<Vec<_>>())?;
    #[cfg(windows)]
    let extensions = if requested.extension().is_some() {
        vec![String::new()]
    } else {
        env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    #[cfg(not(windows))]
    let extensions = vec![String::new()];

    directories.into_iter().find_map(|directory| {
        extensions.iter().find_map(|extension| {
            let candidate = directory.join(format!("{program}{extension}"));
            candidate.is_file().then_some(candidate)
        })
    })
}

fn truncate_owned(value: String, max_chars: usize) -> String {
    value
        .char_indices()
        .nth(max_chars)
        .map_or(value.clone(), |(index, _)| value[..index].to_owned())
}
