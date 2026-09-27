use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use uuid::Uuid;

use crate::task::TaskStatus;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub path: String,
    pub remote_url: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub archived: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub task_id: Option<Uuid>,
    pub task_status: Option<TaskStatus>,
}

pub async fn existing_repository(path: &str) -> Result<(String, String)> {
    let root = repository_root(path).await?;
    let name = project_name(&root);
    Ok((root, name))
}

pub async fn create_repository(path: &str) -> Result<(String, String)> {
    let requested = normalized_path(path)?;
    if requested.exists() && requested.read_dir()?.next().is_some() {
        bail!("The folder for the new repository must be empty")
    }
    std::fs::create_dir_all(&requested)
        .with_context(|| format!("failed to create folder {}", requested.display()))?;
    let output = Command::new("git")
        .args(["init", "--initial-branch=main"])
        .arg(&requested)
        .output()
        .await
        .context("failed to run git init")?;
    if !output.status.success() {
        bail!(
            "git init: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    let root = requested.to_string_lossy().into_owned();
    let name = project_name(&root);
    Ok((root, name))
}

pub async fn clone_repository(url: &str, destination: &str) -> Result<(String, String)> {
    let url = url.trim();
    if url.is_empty() {
        bail!("Git URL is not specified")
    }
    let destination = normalized_path(destination)?;
    if destination.exists() && destination.read_dir()?.next().is_some() {
        bail!("The destination folder must be empty")
    }
    let output = Command::new("git")
        .arg("clone")
        .arg("--")
        .arg(url)
        .arg(&destination)
        .output()
        .await
        .context("failed to run git clone")?;
    if !output.status.success() {
        bail!(
            "git clone: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    let root = destination.to_string_lossy().into_owned();
    let name = project_name(&root);
    Ok((root, name))
}

async fn repository_root(path: &str) -> Result<String> {
    let path = normalized_path(path)?;
    let output = Command::new("git")
        .args(["-C"])
        .arg(&path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .await
        .context("failed to run git")?;
    if !output.status.success() {
        bail!(
            "the selected folder is not a Git repository: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn normalized_path(path: &str) -> Result<PathBuf> {
    let path = path.trim();
    if path.is_empty() {
        bail!("Repository path is not specified")
    }
    let path = Path::new(path);
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn project_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Repository")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn creates_adds_and_clones_repositories() {
        let directory = tempfile::tempdir().unwrap();
        let empty = directory.path().join("empty-repo");
        let (empty_path, empty_name) = create_repository(empty.to_str().unwrap()).await.unwrap();
        assert_eq!(empty_name, "empty-repo");
        assert_eq!(
            Path::new(&existing_repository(&empty_path).await.unwrap().0)
                .canonicalize()
                .unwrap(),
            Path::new(&empty_path).canonicalize().unwrap()
        );

        let source = directory.path().join("source");
        create_repository(source.to_str().unwrap()).await.unwrap();
        std::fs::write(source.join("README.md"), "test\n").unwrap();
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&source)
                .output()
                .unwrap();
            assert!(output.status.success());
        };
        run(&["add", "README.md"]);
        run(&[
            "-c",
            "user.name=CodexBridge Test",
            "-c",
            "user.email=test@localhost",
            "commit",
            "-q",
            "-m",
            "base",
        ]);
        let clone = directory.path().join("clone");
        let (clone_path, clone_name) =
            clone_repository(source.to_str().unwrap(), clone.to_str().unwrap())
                .await
                .unwrap();
        assert_eq!(clone_name, "clone");
        assert!(Path::new(&clone_path).join("README.md").is_file());
    }
}
