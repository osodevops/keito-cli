use colored::Colorize;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::skill::{SkillAgent, SkillCommand, SkillSubcommand, SkillTeamMode};
use crate::cli::GlobalFlags;
use crate::config::ResolvedAuth;
use crate::error::AppError;
use crate::output::OutputMode;

const SKILL_NAME: &str = "keito-time-track";
const DEFAULT_SKILLS_PACKAGE: &str = "skills@1.5.6";
const BUNDLED_SKILL_SOURCE: &str = "bundled";

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

#[derive(Debug)]
struct BundledSkillFile {
    path: &'static str,
    contents: &'static str,
    executable: bool,
}

const BUNDLED_SKILL_FILES: &[BundledSkillFile] = &[
    BundledSkillFile {
        path: "SKILL.md",
        contents: include_str!("../../skills/keito-time-track/SKILL.md"),
        executable: false,
    },
    BundledSkillFile {
        path: "agents/openai.yaml",
        contents: include_str!("../../skills/keito-time-track/agents/openai.yaml"),
        executable: false,
    },
    BundledSkillFile {
        path: "assets/claude.md-block.md",
        contents: include_str!("../../skills/keito-time-track/assets/claude.md-block.md"),
        executable: false,
    },
    BundledSkillFile {
        path: "assets/config.example.yml",
        contents: include_str!("../../skills/keito-time-track/assets/config.example.yml"),
        executable: false,
    },
    BundledSkillFile {
        path: "hooks/lib/config.sh",
        contents: include_str!("../../skills/keito-time-track/hooks/lib/config.sh"),
        executable: false,
    },
    BundledSkillFile {
        path: "hooks/lib/duration.sh",
        contents: include_str!("../../skills/keito-time-track/hooks/lib/duration.sh"),
        executable: false,
    },
    BundledSkillFile {
        path: "hooks/lib/log.sh",
        contents: include_str!("../../skills/keito-time-track/hooks/lib/log.sh"),
        executable: false,
    },
    BundledSkillFile {
        path: "hooks/session-end.sh",
        contents: include_str!("../../skills/keito-time-track/hooks/session-end.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "hooks/session-start.sh",
        contents: include_str!("../../skills/keito-time-track/hooks/session-start.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "installers/install-claude-code.sh",
        contents: include_str!("../../skills/keito-time-track/installers/install-claude-code.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "installers/install-codex.sh",
        contents: include_str!("../../skills/keito-time-track/installers/install-codex.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "scripts/disable.sh",
        contents: include_str!("../../skills/keito-time-track/scripts/disable.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "scripts/install-cli.sh",
        contents: include_str!("../../skills/keito-time-track/scripts/install-cli.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "scripts/pause-resume.sh",
        contents: include_str!("../../skills/keito-time-track/scripts/pause-resume.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "scripts/setup-wizard.sh",
        contents: include_str!("../../skills/keito-time-track/scripts/setup-wizard.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "scripts/status.sh",
        contents: include_str!("../../skills/keito-time-track/scripts/status.sh"),
        executable: true,
    },
];

struct MaterializedBundledSkill {
    temp_root: PathBuf,
    skill_root: PathBuf,
}

impl Drop for MaterializedBundledSkill {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_root);
    }
}

#[derive(Debug, Serialize)]
struct TeamInitStatus {
    mode: String,
    repo_root: String,
    changed_files: Vec<String>,
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
        SkillSubcommand::TeamInit { mode: team_mode } => team_init(team_mode, global, mode).await,
    }
}

pub async fn install_defaults(global: &GlobalFlags, mode: OutputMode) -> Result<(), AppError> {
    let source =
        std::env::var("KEITO_SKILL_SOURCE").unwrap_or_else(|_| BUNDLED_SKILL_SOURCE.into());
    install(global, mode, &source, default_agents(), false).await
}

async fn install(
    global: &GlobalFlags,
    mode: OutputMode,
    source: &str,
    agents: Vec<SkillAgent>,
    skip_skills_add: bool,
) -> Result<(), AppError> {
    let source = source.trim();
    let use_bundled = source.is_empty() || source == BUNDLED_SKILL_SOURCE;

    if !skip_skills_add && !use_bundled && find_in_path("npx").is_none() {
        return Err(AppError::Config(
            "npx is required for an external skill source. Install Node.js, use --source bundled, or run the manual skill installer.".into(),
        ));
    }
    if find_in_path("jq").is_none() {
        return Err(AppError::Config(
            "jq is required by the Keito Skill hook installers and runtime scripts.".into(),
        ));
    }

    let bundled_skill = if !skip_skills_add && use_bundled {
        Some(materialize_bundled_skill()?)
    } else {
        None
    };

    for agent in &agents {
        let show_child_output = !global.quiet && mode == OutputMode::Table;
        if skip_skills_add {
            run_hook_installer(*agent, show_child_output)?;
        } else if let Some(skill) = bundled_skill.as_ref() {
            run_bundled_hook_installer(*agent, show_child_output, &skill.skill_root)?;
        } else {
            run_skills_add(source, *agent, show_child_output)?;
            run_hook_installer(*agent, show_child_output)?;
        }
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
        println!("For shared repos: keito skill team-init optional");
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

fn run_bundled_hook_installer(
    agent: SkillAgent,
    show_output: bool,
    skill_root: &Path,
) -> Result<(), AppError> {
    let installer_name = match agent {
        SkillAgent::Codex => "install-codex.sh",
        SkillAgent::ClaudeCode => "install-claude-code.sh",
    };
    let script = skill_root.join("installers").join(installer_name);
    run_installer_script(agent, show_output, &script)
}

fn run_hook_installer(agent: SkillAgent, show_output: bool) -> Result<(), AppError> {
    let script = hook_installer_path(agent).ok_or_else(|| {
        AppError::Config(format!(
            "Keito Skill files were not found for {} after install",
            agent.display_name()
        ))
    })?;

    run_installer_script(agent, show_output, &script)
}

fn run_installer_script(
    agent: SkillAgent,
    show_output: bool,
    script: &Path,
) -> Result<(), AppError> {
    let current_exe = std::env::current_exe()
        .map_err(|e| AppError::Config(format!("Could not resolve current keito binary: {e}")))?;

    let status = ProcessCommand::new("bash")
        .arg(script)
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

fn materialize_bundled_skill() -> Result<MaterializedBundledSkill, AppError> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temp_root =
        std::env::temp_dir().join(format!("keito-skill-{}-{unique}", std::process::id()));
    let skill_root = temp_root.join(SKILL_NAME);

    for file in BUNDLED_SKILL_FILES {
        let path = skill_root.join(file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                AppError::Config(format!("Failed to create bundled skill directory: {e}"))
            })?;
        }
        fs::write(&path, file.contents).map_err(|e| {
            AppError::Config(format!(
                "Failed to write bundled skill file {}: {e}",
                file.path
            ))
        })?;
        set_executable_if_needed(&path, file.executable)?;
    }

    Ok(MaterializedBundledSkill {
        temp_root,
        skill_root,
    })
}

#[cfg(unix)]
fn set_executable_if_needed(path: &Path, executable: bool) -> Result<(), AppError> {
    if !executable {
        return Ok(());
    }

    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|e| AppError::Config(format!("Failed to inspect {}: {e}", path.display())))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|e| {
        AppError::Config(format!("Failed to mark {} executable: {e}", path.display()))
    })?;

    Ok(())
}

#[cfg(not(unix))]
fn set_executable_if_needed(_path: &Path, _executable: bool) -> Result<(), AppError> {
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

async fn team_init(
    team_mode: SkillTeamMode,
    global: &GlobalFlags,
    mode: OutputMode,
) -> Result<(), AppError> {
    let repo_root = git_repo_root()?;
    let mut changed_files = Vec::new();

    let ag_sections = team_guidance_sections(team_mode);
    append_section_once(
        &repo_root.join("AGENTS.md"),
        "## Keito Time Tracking",
        ag_sections.agents,
        &mut changed_files,
    )?;
    append_section_once(
        &repo_root.join("CLAUDE.md"),
        "## Keito Time Tracking",
        ag_sections.claude,
        &mut changed_files,
    )?;
    ensure_gitignore(&repo_root, &mut changed_files)?;
    write_config_example(&repo_root, &mut changed_files)?;

    if global.quiet {
        return Ok(());
    }

    let status = TeamInitStatus {
        mode: match team_mode {
            SkillTeamMode::Optional => "optional".to_string(),
            SkillTeamMode::Required => "required".to_string(),
        },
        repo_root: repo_root.display().to_string(),
        changed_files,
    };

    if mode == OutputMode::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status)
                .map_err(|e| AppError::ServerError(format!("JSON serialization failed: {e}")))?
        );
    } else {
        println!(
            "{} Keito repo guidance initialized ({})",
            "Done.".green().bold(),
            status.mode
        );
        if status.changed_files.is_empty() {
            println!("No files changed; Keito guidance was already present.");
        } else {
            println!("Changed files:");
            for file in &status.changed_files {
                println!("- {file}");
            }
        }
        println!("Next: commit the guidance, then run /track-time-keito in this repo.");
    }

    Ok(())
}

struct TeamGuidance {
    agents: &'static str,
    claude: &'static str,
}

fn team_guidance_sections(team_mode: SkillTeamMode) -> TeamGuidance {
    match team_mode {
        SkillTeamMode::Optional => TeamGuidance {
            agents: "\
## Keito Time Tracking

This repository can use Keito agent time tracking for billable coding work.
If the user wants tracking enabled, verify `keito skill status --json`, then
run `/track-time-keito` from Codex or Claude Code to create the repo-local
`.keito/config.yml` project/task mapping.

Do not commit `.keito/config.yml`; it is specific to one Keito account,
client, project, and task. The lifecycle hooks create one `source=agent` time
entry when a tracked session ends, so do not create duplicate manual entries.
",
            claude: "\
## Keito Time Tracking

This repository can use Keito agent time tracking for billable coding work.
If the user wants tracking enabled, verify `keito skill status --json`, then
run `/track-time-keito` to create the repo-local `.keito/config.yml`
project/task mapping.

Do not commit `.keito/config.yml`; it is specific to one Keito account,
client, project, and task. The lifecycle hooks create one `source=agent` time
entry when a tracked session ends, so do not create duplicate manual entries.
",
        },
        SkillTeamMode::Required => TeamGuidance {
            agents: "\
## Keito Time Tracking

Keito agent time tracking is required before billable coding work in this
repository. Before starting, verify `keito skill status --json`. If the skill
or hooks are missing, stop and tell the user to run `keito skill install`.

If `.keito/config.yml` is missing for this worktree, stop and run
`/track-time-keito` from Codex or Claude Code so the user can choose the Keito
client, project, and task. Do not guess project or task IDs. Do not commit
`.keito/config.yml`.

The lifecycle hooks create one `source=agent` time entry when a tracked session
ends. Do not create duplicate manual entries for the same coding session.
",
            claude: "\
## Keito Time Tracking

Keito agent time tracking is required before billable coding work in this
repository. Before starting, verify `keito skill status --json`. If the skill
or hooks are missing, stop and tell the user to run `keito skill install`.

If `.keito/config.yml` is missing for this worktree, stop and run
`/track-time-keito` so the user can choose the Keito client, project, and task.
Do not guess project or task IDs. Do not commit `.keito/config.yml`.

The lifecycle hooks create one `source=agent` time entry when a tracked session
ends. Do not create duplicate manual entries for the same coding session.
",
        },
    }
}

fn git_repo_root() -> Result<PathBuf, AppError> {
    let output = ProcessCommand::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| AppError::Config(format!("Failed to run git: {e}")))?;

    if !output.status.success() {
        return Err(AppError::Config(
            "Run this from inside the Git repository you want to bootstrap.".into(),
        ));
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        return Err(AppError::Config(
            "Git did not return a repository root.".into(),
        ));
    }

    Ok(PathBuf::from(root))
}

fn append_section_once(
    path: &Path,
    marker: &str,
    section: &str,
    changed_files: &mut Vec<String>,
) -> Result<(), AppError> {
    let current = fs::read_to_string(path).unwrap_or_default();
    if current.contains(marker) {
        return Ok(());
    }

    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    if !next.is_empty() {
        next.push('\n');
    }
    next.push_str(section.trim_end());
    next.push('\n');

    fs::write(path, next)
        .map_err(|e| AppError::Config(format!("Failed to update {}: {e}", path.display())))?;
    changed_files.push(relative_display(path));
    Ok(())
}

fn ensure_gitignore(repo_root: &Path, changed_files: &mut Vec<String>) -> Result<(), AppError> {
    let path = repo_root.join(".gitignore");
    let mut current = fs::read_to_string(&path).unwrap_or_default();
    let mut changed = false;
    for line in [
        ".keito/config.yml",
        ".keito/*.disabled*",
        "!.keito/config.example.yml",
    ] {
        if !current.lines().any(|existing| existing.trim() == line) {
            if !current.is_empty() && !current.ends_with('\n') {
                current.push('\n');
            }
            current.push_str(line);
            current.push('\n');
            changed = true;
        }
    }

    if changed {
        fs::write(&path, current)
            .map_err(|e| AppError::Config(format!("Failed to update {}: {e}", path.display())))?;
        changed_files.push(relative_display(&path));
    }

    Ok(())
}

fn write_config_example(repo_root: &Path, changed_files: &mut Vec<String>) -> Result<(), AppError> {
    let path = repo_root.join(".keito").join("config.example.yml");
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| AppError::Config(format!("Failed to create {}: {e}", parent.display())))?;
    }
    fs::write(
        &path,
        include_str!("../../skills/keito-time-track/assets/config.example.yml"),
    )
    .map_err(|e| AppError::Config(format!("Failed to write {}: {e}", path.display())))?;
    changed_files.push(relative_display(&path));
    Ok(())
}

fn relative_display(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            if name == "config.example.yml" {
                ".keito/config.example.yml".to_string()
            } else {
                name.to_string()
            }
        })
        .unwrap_or_else(|| path.display().to_string())
}
