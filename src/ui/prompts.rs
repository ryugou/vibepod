use anyhow::Result;
use dialoguer::{Confirm, Select};

pub fn select_agent() -> Result<String> {
    let items = vec![
        "Claude Code",
        "Gemini CLI (coming soon)",
        "OpenAI Codex (coming soon)",
    ];

    let selection = Select::new()
        .with_prompt("Which AI coding agent do you use?")
        .items(&items)
        .default(0)
        .interact()?;

    match selection {
        0 => Ok("claude".to_string()),
        _ => {
            println!("This agent is not yet supported. Using Claude Code.");
            Ok("claude".to_string())
        }
    }
}

pub fn confirm_project_registration(project_name: &str) -> Result<bool> {
    let result = Confirm::new()
        .with_prompt(format!(
            "First time running in '{}'. Register it?",
            project_name
        ))
        .default(true)
        .interact()?;
    Ok(result)
}

pub fn handle_existing_container(container_name: &str) -> Result<ExistingContainerAction> {
    let items = vec![
        "Attach to existing container",
        "Stop and start new container",
    ];

    let selection = Select::new()
        .with_prompt(format!("Container '{}' is already running", container_name))
        .items(&items)
        .default(0)
        .interact()?;

    match selection {
        0 => Ok(ExistingContainerAction::Attach),
        _ => Ok(ExistingContainerAction::Replace),
    }
}

pub enum ExistingContainerAction {
    Attach,
    Replace,
}
