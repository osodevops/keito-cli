use colored::Colorize;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};

use crate::cli::skill::{SkillAgent, SkillCommand, SkillSubcommand};
use crate::cli::GlobalFlags;
use crate::config::ResolvedAuth;
use crate::error::AppError;
use crate::output::OutputMode;

const SKILL_NAME: &str = "keito-time-track";
const DEFAULT_SKILLS_PACKAGE: &str = "skills@1.5.6";

#[derive(Debug, Serialize)]
struct SkillStatus {
    cli_installed: bool,
    cli_path: Option<String>,
    npx_installed: bool,
    jq_installed: bool,
    authenticated: bool,
    account_id: Option<String>,
    codex: AgentStatus,
    claude_code: AgentStatus,
}

#[derive(Debug, Serialize)]
struct AgentStatus {
    skill_installed: bool,
    skill_path: Option<String>,
    hooks_configured: bool,
    hook_config_path: Option<String>,
}

pub async fn run(
    cmd: SkillCommand,
    global: &GlobalFlags,
    mode: OutputMode,
) -> Result<(), AppError> {
    match cmd.command {
        SkillSubcommand::Install {
            source,
            agent,
            skip_skills_add,
        } => {
            install(
                global,
                mode,
                &source,
                selected_agents(agent),
                skip_skills_add,
            )
            .await
        }
        SkillSubcommand::Status => status(global, mode, false).await,
        SkillSubcommand::Doctor => status(global, mode, true).await,
    }
}

pub async fn install_defaults(global: &GlobalFlags, mode: OutputMode) -> Result<(), AppError> {
    let source =
        std::env::var("KEITO_SKILL_SOURCE").unwrap_or_else(|_| "keito-ai/keito-skill".into());
    install(global, mode, &source, default_agents(), false).await
}

async fn install(
    global: &GlobalFlags,
    mode: OutputMode,
    source: &str,
    agents: Vec<SkillAgent>,
    skip_skills_add: bool,
) -> Result<(), AppError> {
    if !skip_skills_add && find_in_path("npx").is_none() {
        return Err(AppError::Config(
            "npx is required to install the Keito Skill. Install Node.js or run the manual skill installer.".into(),
        ));
    }
    if find_in_path("jq").is_none() {
        return Err(AppError::Config(
            "jq is required by the Keito Skill hook installers and runtime scripts.".into(),
        ));
    }

    for agent in &agents {
        let show_child_output = !global.quiet && mode == OutputMode::Table;
        if !skip_skills_add {
            run_skills_add(source, *agent, show_child_output)?;
        }
        run_hook_installer(*agent, show_child_output)?;
    }

    if global.quiet {
        return Ok(());
    }

    let current = collect_status(global);
    if mode == OutputMode::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&current)
                .map_err(|e| AppError::ServerError(format!("JSON serialization failed: {e}")))?
        );
    } else {
        println!("{}", "Keito Skill installed.".green().bold());
        println!("Next: cd into each client repo and run /track-time-keito.");
        println!("Check readiness any time with: keito skill doctor");
    }

    Ok(())
}

async fn status(global: &GlobalFlags, mode: OutputMode, doctor: bool) -> Result<(), AppError> {
    let current = collect_status(global);

    if global.quiet {
        return Ok(());
    }

    if mode == OutputMode::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&current)
                .map_err(|e| AppError::ServerError(format!("JSON serialization failed: {e}")))?
        );
        return Ok(());
    }

    print_status(&current, doctor);
    Ok(())
}

fn selected_agents(agents: Vec<SkillAgent>) -> Vec<SkillAgent> {
    if agents.is_empty() {
        default_agents()
    } else {
        agents
    }
}

fn default_agents() -> Vec<SkillAgent> {
    vec![SkillAgent::Codex, SkillAgent::ClaudeCode]
}

fn run_skills_add(source: &str, agent: SkillAgent, show_output: bool) -> Result<(), AppError> {
    let skills_package =
        std::env::var("KEITO_SKILLS_PACKAGE").unwrap_or_else(|_| DEFAULT_SKILLS_PACKAGE.into());
    let status = ProcessCommand::new("npx")
        .args([
            "--yes",
            &skills_package,
            "add",
            source,
            "-g",
            "-a",
            agent.skills_cli_name(),
            "-s",
            SKILL_NAME,
            "-y",
            "--copy",
        ])
        .stdin(Stdio::null())
        .stdout(child_stdio(show_output))
        .stderr(child_stdio(show_output))
        .status()
        .map_err(|e| AppError::Config(format!("Failed to run the skills installer: {e}")))?;

    if !status.success() {
        return Err(AppError::Config(format!(
            "Skills installer failed for {}",
            agent.display_name()
        )));
    }

    Ok(())
}

fn run_hook_installer(agent: SkillAgent, show_output: bool) -> Result<(), AppError> {
    let script = hook_installer_path(agent).ok_or_else(|| {
        AppError::Config(format!(
            "Keito Skill files were not found for {} after install",
            agent.display_name()
        ))
    })?;

    let current_exe = std::env::current_exe()
        .map_err(|e| AppError::Config(format!("Could not resolve current keito binary: {e}")))?;

    let status = ProcessCommand::new("bash")
        .arg(&script)
        .env("KEITO_CLI_BIN", current_exe)
        .stdin(Stdio::null())
        .stdout(child_stdio(show_output))
        .stderr(child_stdio(show_output))
        .status()
        .map_err(|e| AppError::Config(format!("Failed to run hook installer: {e}")))?;

    if !status.success() {
        return Err(AppError::Config(format!(
            "Hook installer failed for {}",
            agent.display_name()
        )));
    }

    Ok(())
}

fn child_stdio(show_output: bool) -> Stdio {
    if show_output {
        Stdio::inherit()
    } else {
        Stdio::null()
    }
}

fn hook_installer_path(agent: SkillAgent) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let candidates = match agent {
        SkillAgent::Codex => vec![
            home.join(".agents/skills").join(SKILL_NAME),
            home.join(".codex/skills").join(SKILL_NAME),
        ],
        SkillAgent::ClaudeCode => vec![home.join(".claude/skills").join(SKILL_NAME)],
    };

    let installer_name = match agent {
        SkillAgent::Codex => "install-codex.sh",
        SkillAgent::ClaudeCode => "install-claude-code.sh",
    };

    candidates
        .into_iter()
        .map(|root| root.join("installers").join(installer_name))
        .find(|path| path.exists())
}

fn collect_status(global: &GlobalFlags) -> SkillStatus {
    let auth = ResolvedAuth::resolve(global).ok();
    SkillStatus {
        cli_installed: true,
        cli_path: std::env::current_exe()
            .ok()
            .map(|path| path.display().to_string()),
        npx_installed: find_in_path("npx").is_some(),
        jq_installed: find_in_path("jq").is_some(),
        authenticated: auth.is_some(),
        account_id: auth.map(|auth| auth.workspace_id),
        codex: agent_status(SkillAgent::Codex),
        claude_code: agent_status(SkillAgent::ClaudeCode),
    }
}

fn agent_status(agent: SkillAgent) -> AgentStatus {
    let home = dirs::home_dir();
    let skill_path = home.as_ref().and_then(|home| match agent {
        SkillAgent::Codex => first_existing(&[
            home.join(".agents/skills").join(SKILL_NAME),
            home.join(".codex/skills").join(SKILL_NAME),
        ]),
        SkillAgent::ClaudeCode => first_existing(&[home.join(".claude/skills").join(SKILL_NAME)]),
    });
    let hook_config_path = home.as_ref().map(|home| match agent {
        SkillAgent::Codex => home.join(".codex/hooks.json"),
        SkillAgent::ClaudeCode => home.join(".claude/settings.json"),
    });
    let hooks_configured = hook_config_path
        .as_ref()
        .is_some_and(|path| hook_configured(path));

    AgentStatus {
        skill_installed: skill_path.is_some(),
        skill_path: skill_path.map(|path| path.display().to_string()),
        hooks_configured,
        hook_config_path: hook_config_path.map(|path| path.display().to_string()),
    }
}

fn first_existing(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|path| path.exists()).cloned()
}

fn hook_configured(path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    contents.contains(SKILL_NAME)
        && contents.contains("session-start.sh")
        && contents.contains("session-end.sh")
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    if binary.contains(std::path::MAIN_SEPARATOR) {
        let path = PathBuf::from(binary);
        return path.exists().then_some(path);
    }

    let path_var = std::env::var_os("PATH")?;
    let names = executable_names(binary);
    std::env::split_paths(&path_var)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

fn executable_names(binary: &str) -> Vec<String> {
    #[cfg(not(windows))]
    {
        vec![binary.to_string()]
    }
    #[cfg(windows)]
    {
        let mut names = vec![binary.to_string()];
        if Path::new(binary).extension().is_none() {
            let pathext =
                std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
            names.extend(
                pathext
                    .split(';')
                    .filter(|ext| !ext.is_empty())
                    .map(|ext| format!("{binary}{ext}")),
            );
        }
        names
    }
}

fn print_status(status: &SkillStatus, doctor: bool) {
    println!("Keito CLI: {}", yes_no(status.cli_installed));
    if let Some(path) = &status.cli_path {
        println!("CLI path: {path}");
    }
    println!("npx available: {}", yes_no(status.npx_installed));
    println!("jq available: {}", yes_no(status.jq_installed));
    println!("Authenticated: {}", yes_no(status.authenticated));
    if let Some(account_id) = &status.account_id {
        println!("Account ID: {account_id}");
    }
    print_agent_status("Codex", &status.codex);
    print_agent_status("Claude Code", &status.claude_code);

    if doctor {
        println!();
        println!("Next actions:");
        if !status.npx_installed {
            println!("- Install Node.js/npm so npx is available.");
        }
        if !status.jq_installed {
            println!("- Install jq.");
        }
        if !status.authenticated {
            println!("- Run keito auth login.");
        }
        if !status.codex.hooks_configured && !status.claude_code.hooks_configured {
            println!("- Run keito skill install.");
        }
        if status.authenticated
            && (status.codex.hooks_configured || status.claude_code.hooks_configured)
        {
            println!("- cd into each client repo and run /track-time-keito.");
        }
    }
}

fn print_agent_status(name: &str, status: &AgentStatus) {
    println!(
        "{name} skill: {}, hooks: {}",
        yes_no(status.skill_installed),
        yes_no(status.hooks_configured)
    );
}

fn yes_no(value: bool) -> String {
    if value {
        "yes".green().to_string()
    } else {
        "no".red().to_string()
    }
}
