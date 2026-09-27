use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryInfo {
    pub root: String,
    pub head: String,
    pub branch: Option<String>,
    pub dirty: bool,
}

pub async fn inspect(workspace: &str) -> Result<RepositoryInfo> {
    let root = git_output(workspace, &["rev-parse", "--show-toplevel"]).await?;
    let head = git_output(&root, &["rev-parse", "HEAD"]).await?;
    let branch = git_output(&root, &["branch", "--show-current"])
        .await
        .ok()
        .filter(|value| !value.is_empty());
    let dirty = !git_output(&root, &["status", "--porcelain"])
        .await?
        .is_empty();
    Ok(RepositoryInfo {
        root,
        head,
        branch,
        dirty,
    })
}

pub async fn create_worktree(
    repository: &str,
    base_commit: &str,
    worktree_root: &Path,
    task_id: Uuid,
) -> Result<PathBuf> {
    std::fs::create_dir_all(worktree_root).context("failed to create the worktrees directory")?;
    let path = worktree_root.join(task_id.to_string());
    if path.is_dir() {
        return Ok(path);
    }
    let output = Command::new("git")
        .args(["-C", repository, "worktree", "add", "--detach"])
        .arg(&path)
        .arg(base_commit)
        .output()
        .await
        .context("failed to run git worktree add")?;
    if !output.status.success() {
        bail!(
            "git worktree add: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(path)
}

pub async fn diff(worktree: &Path) -> Result<String> {
    let intent_to_add = Command::new("git")
        .args(["add", "--intent-to-add", "--all", "--"])
        .current_dir(worktree)
        .output()
        .await
        .context("failed to stage new files for git diff")?;
    if !intent_to_add.status.success() {
        bail!(
            "git add --intent-to-add: {}",
            String::from_utf8_lossy(&intent_to_add.stderr).trim()
        );
    }
    let output = Command::new("git")
        .args(["diff", "--no-ext-diff", "--binary", "HEAD"])
        .current_dir(worktree)
        .output()
        .await
        .context("failed to obtain git diff")?;
    if !output.status.success() {
        bail!(
            "git diff: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn git_output(workspace: &str, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .await
        .context("failed to run git")?;
    if !output.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn diff_includes_untracked_files() {
        let directory = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(directory.path())
                .output()
                .unwrap();
            assert!(output.status.success());
        };
        run(&["init", "-q"]);
        std::fs::write(directory.path().join("tracked.txt"), "base\n").unwrap();
        run(&["add", "tracked.txt"]);
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
        std::fs::write(directory.path().join("new.txt"), "new content\n").unwrap();

        let result = diff(directory.path()).await.unwrap();

        assert!(result.contains("new.txt"));
        assert!(result.contains("new content"));
    }
}
