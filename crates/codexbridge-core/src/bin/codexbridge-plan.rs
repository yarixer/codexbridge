use codexbridge_core::{codex, git, task::TaskSpec};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut arguments = std::env::args().skip(1);
    let workspace = arguments.next().unwrap_or_else(|| ".".into());
    let goal = arguments
        .next()
        .unwrap_or_else(|| "Describe a minimal, safe improvement to this project".into());
    let repository = git::inspect(&workspace).await?;
    let spec = TaskSpec {
        goal,
        workspace: repository.root,
        base_commit: repository.head,
        constraints: vec!["Do not modify files during planning".into()],
        acceptance_criteria: vec![],
        validation_commands: vec![],
        max_worker_attempts: 1,
    };
    let plan = codex::plan_task(&spec).await?;
    println!("{}", serde_json::to_string_pretty(&plan)?);
    Ok(())
}
