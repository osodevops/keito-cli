use colored::Colorize;
use dialoguer::MultiSelect;
use serde::Serialize;
use std::fs;
use std::io::IsTerminal;
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
    capture_runtime: String,
    credentials_configured: bool,
    authenticated: bool,
    authentication_state: String,
    hook_trust: String,
    delivery_verified: bool,
    ready: bool,
    capture: serde_json::Value,
    worker: serde_json::Value,
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
    guard_runtime: String,
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
        path: "hooks/claude-plugin.json",
        contents: include_str!("../../skills/keito-time-track/hooks/claude-plugin.json"),
        executable: false,
    },
    BundledSkillFile {
        path: "hooks/dispatch.sh",
        contents: include_str!("../../skills/keito-time-track/hooks/dispatch.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "hooks/interrupt.sh",
        contents: include_str!("../../skills/keito-time-track/hooks/interrupt.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "hooks/prompt.sh",
        contents: include_str!("../../skills/keito-time-track/hooks/prompt.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "hooks/session-end.sh",
        contents: include_str!("../../skills/keito-time-track/hooks/session-end.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "hooks/session-finalize.sh",
        contents: include_str!("../../skills/keito-time-track/hooks/session-finalize.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "hooks/session-start.sh",
        contents: include_str!("../../skills/keito-time-track/hooks/session-start.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "hooks/stop.sh",
        contents: include_str!("../../skills/keito-time-track/hooks/stop.sh"),
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
        path: "installers/install.sh",
        contents: include_str!("../../skills/keito-time-track/installers/install.sh"),
        executable: true,
    },
    BundledSkillFile {
        path: "installers/uninstall.sh",
        contents: include_str!("../../skills/keito-time-track/installers/uninstall.sh"),
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
            confirm_migration,
        } => {
            install(
                global,
                mode,
                &source,
                selected_agents(agent)?,
                skip_skills_add,
                confirm_migration,
            )
            .await
        }
        SkillSubcommand::Uninstall { agent } => {
            uninstall(selected_agents(agent)?, global, mode).await
        }
        SkillSubcommand::Runtime => {
            let runtime = crate::capture_runtime_pair::current()?;
            if !global.quiet {
                println!(
                    "{}",
                    serde_json::to_string(&runtime)
                        .map_err(|_| AppError::Config("Cannot render runtime identity".into()))?
                );
            }
            Ok(())
        }
        SkillSubcommand::Status => status(global, mode, false, false).await,
        SkillSubcommand::Doctor { live } => status(global, mode, true, live).await,
        SkillSubcommand::TeamInit { mode: team_mode } => team_init(team_mode, global, mode).await,
    }
}

pub async fn install_defaults(global: &GlobalFlags, mode: OutputMode) -> Result<(), AppError> {
    let source =
        std::env::var("KEITO_SKILL_SOURCE").unwrap_or_else(|_| BUNDLED_SKILL_SOURCE.into());
    install(
        global,
        mode,
        &source,
        selected_agents(vec![])?,
        false,
        false,
    )
    .await
}

async fn install(
    global: &GlobalFlags,
    mode: OutputMode,
    source: &str,
    agents: Vec<SkillAgent>,
    skip_skills_add: bool,
    confirm_migration: bool,
) -> Result<(), AppError> {
    let source = source.trim();
    let use_bundled = source.is_empty() || source == BUNDLED_SKILL_SOURCE;

    if !skip_skills_add && !use_bundled && find_in_path("npx").is_none() {
        return Err(AppError::Config(
            "npx is required for an external skill source. Install Node.js, use --source bundled, or run the manual skill installer.".into(),
        ));
    }
    // Qualification precedes every profile mutation, including external
    // skills downloads. An arbitrary adjacent binary cannot qualify capture.
    let runtime = crate::capture_runtime_pair::current()?;
    let home =
        dirs::home_dir().ok_or_else(|| AppError::Config("Cannot identify profile home".into()))?;
    let bundled_skill = if !skip_skills_add && use_bundled {
        Some(materialize_bundled_skill()?)
    } else {
        None
    };
    for agent in &agents {
        if !skip_skills_add && !use_bundled {
            run_skills_add(source, *agent, !global.quiet && mode == OutputMode::Table)?;
        }
        let root = if let Some(skill) = bundled_skill.as_ref() {
            skill.skill_root.clone()
        } else {
            hook_installer_path(*agent)
                .and_then(|path| path.parent()?.parent().map(Path::to_path_buf))
                .ok_or_else(|| AppError::Config("Keito skill source is unavailable".into()))?
        };
        crate::skill_adapter::install(crate::skill_adapter::InstallOptions {
            source_root: &root,
            profile_home: &home,
            agent: *agent,
            runtime: &runtime,
            confirm_migration,
            harness_version: None,
        })?;
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

async fn status(
    global: &GlobalFlags,
    mode: OutputMode,
    doctor: bool,
    live: bool,
) -> Result<(), AppError> {
    let mut current = collect_status(global);
    if live {
        let auth = ResolvedAuth::resolve(global)?;
        let config = crate::config::AppConfig::load()?;
        let client = crate::api::client::KeitorClient::new(&auth, &config.api_base_url())?;
        match client.get_me().await {
            Ok(me) if !me.is_personal_read_only_sync() => {
                current.authenticated = true;
                current.authentication_state = "VERIFIED".into();
                let repository = &mut current.capture["current_repository"];
                let matches = repository["approved"] == true
                    && repository["mapping_current"] == true
                    && repository["paused"] != true
                    && repository["workspace_id"].as_str() == Some(auth.workspace_id.as_str())
                    && repository["owner_id"].as_str() == Some(me.id.as_str())
                    && repository["origin_hash"].as_str()
                        == Some(
                            crate::capture::hash(config.api_base_url().trim_end_matches('/'))
                                .as_str(),
                        );
                repository["authentication_binding"] =
                    serde_json::json!(if matches { "MATCHED" } else { "MISMATCH" });
                current.delivery_verified =
                    matches && repository["delivery_history_verified"] == true;
                current.capture["delivery_verified"] = serde_json::json!(current.delivery_verified);
            }
            Ok(_) => {
                current.authentication_state = "INCOMPATIBLE_CREDENTIAL".into();
            }
            Err(_) => {
                current.authentication_state = "NEEDS_AUTH_OR_NETWORK".into();
            }
        }
    }

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

fn selected_agents(agents: Vec<SkillAgent>) -> Result<Vec<SkillAgent>, AppError> {
    if !agents.is_empty() {
        let mut selected = Vec::new();
        for agent in agents {
            if !selected.contains(&agent) {
                selected.push(agent);
            }
        }
        return Ok(selected);
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(AppError::InvalidInput(
            "Select explicit adapter targets: --agent claude-code and/or --agent codex".into(),
        ));
    }
    let selected = MultiSelect::new()
        .with_prompt("Install which coding-agent adapters?")
        .items(["Claude Code", "Codex"])
        .interact()
        .map_err(|_| AppError::Config("Cannot read adapter target selection".into()))?;
    if selected.is_empty() {
        return Err(AppError::InvalidInput(
            "No adapters selected; nothing installed".into(),
        ));
    }
    Ok(selected
        .into_iter()
        .map(|i| {
            if i == 0 {
                SkillAgent::ClaudeCode
            } else {
                SkillAgent::Codex
            }
        })
        .collect())
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

fn materialize_bundled_skill() -> Result<MaterializedBundledSkill, AppError> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temp_root =
        std::env::temp_dir().join(format!("keito-skill-{}-{unique}", std::process::id()));
    let skill_root = temp_root.join(SKILL_NAME);
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    let mut builder = builder;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&temp_root)
        .map_err(|_| AppError::Config("Cannot create private bundled skill source".into()))?;

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
    let (capture, worker) = capture_diagnostics(global);
    let delivery_verified = capture
        .get("delivery_verified")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    SkillStatus {
        cli_installed: true,
        cli_path: std::env::current_exe()
            .ok()
            .map(|path| path.display().to_string()),
        npx_installed: find_in_path("npx").is_some(),
        jq_installed: find_in_path("jq").is_some(),
        capture_runtime: if crate::capture_runtime_pair::current().is_ok() {
            "VERIFIED_RELEASE_PAIR"
        } else {
            "UNQUALIFIED_OR_STALE"
        }
        .into(),
        credentials_configured: auth.is_some(),
        authenticated: false,
        authentication_state: "UNVERIFIED".into(),
        hook_trust: "UNKNOWN_REVIEW_IN_HARNESS".into(),
        delivery_verified,
        ready: false,
        capture,
        worker,
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
    let runtime = crate::capture_runtime_pair::current().ok();
    let hooks_configured = home.as_ref().is_some_and(|home| {
        crate::skill_adapter::inspect(home, agent, runtime.as_ref()).unwrap_or(false)
    });

    AgentStatus {
        skill_installed: skill_path.is_some(),
        skill_path: skill_path.map(|path| path.display().to_string()),
        hooks_configured,
        guard_runtime: if runtime.is_some() {
            "VERIFIED_RELEASE_PAIR"
        } else {
            "UNQUALIFIED_OR_STALE"
        }
        .into(),
        hook_config_path: hook_config_path.map(|path| path.display().to_string()),
    }
}

fn first_existing(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|path| path.exists()).cloned()
}

async fn uninstall(
    agents: Vec<SkillAgent>,
    global: &GlobalFlags,
    mode: OutputMode,
) -> Result<(), AppError> {
    let home =
        dirs::home_dir().ok_or_else(|| AppError::Config("Cannot identify profile home".into()))?;
    for agent in agents {
        crate::skill_adapter::uninstall(&home, agent)?;
    }
    status(global, mode, false, false).await
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
    println!("Native capture runtime: {}", status.capture_runtime);
    println!(
        "Credentials configured: {}",
        yes_no(status.credentials_configured)
    );
    println!("Live authentication: {}", status.authentication_state);
    println!("Hook trust: {}", status.hook_trust);
    println!(
        "Capture delivery verified: {}",
        yes_no(status.delivery_verified)
    );
    println!(
        "Repository approval: {}",
        status.capture["current_repository"]["approved"]
            .as_bool()
            .unwrap_or(false)
    );
    println!(
        "Storage paused: {}",
        status.capture["storage_paused"].as_bool().unwrap_or(true)
    );
    println!(
        "Worker installed/loaded/running: {}/{}/{}",
        status.worker["installed"].as_bool().unwrap_or(false),
        status.worker["loaded"].as_bool().unwrap_or(false),
        status.worker["running"].as_bool().unwrap_or(false)
    );
    println!("Ready: no (unknown trust/delivery is not ready)");
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
        if status.capture_runtime != "VERIFIED_RELEASE_PAIR" {
            println!("- Install the official paired Keito release for native capture.");
        }
        if !status.authenticated {
            println!("- Run keito auth login.");
        }
        if !status.codex.hooks_configured && !status.claude_code.hooks_configured {
            println!("- Run keito skill install --agent <claude-code|codex>.");
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
client, project, and task. After explicit local approval, supported turn hooks create
pending-review capture records. Do not create duplicate manual time entries for
captured activity. Review the evidence before approving any client charge.
",
            claude: "\
## Keito Time Tracking

This repository can use Keito agent time tracking for billable coding work.
If the user wants tracking enabled, verify `keito skill status --json`, then
run `/track-time-keito` to create the repo-local `.keito/config.yml`
project/task mapping.

Do not commit `.keito/config.yml`; it is specific to one Keito account,
client, project, and task. After explicit local approval, supported turn hooks create
pending-review capture records. Do not create duplicate manual time entries for
captured activity. Review the evidence before approving any client charge.
",
        },
        SkillTeamMode::Required => TeamGuidance {
            agents: "\
## Keito Time Tracking

Keito agent time tracking is required before billable coding work in this
repository. Before starting, verify `keito skill status --json`. If the skill
or hooks are missing, stop and tell the user to run `keito skill install --agent <claude-code|codex>`.

If `.keito/config.yml` is missing for this worktree, stop and run
`/track-time-keito` from Codex or Claude Code so the user can choose the Keito
client, project, and task. Do not guess project or task IDs. Do not commit
`.keito/config.yml`.

The lifecycle hooks save minimized observed-turn evidence in a private local queue.
A separately supervised worker sends canonical revisions for review; elapsed activity
is not permission to bill. Verify local consent, worker and delivery with
`keito capture status --json` and `keito capture worker-status --json`.
Do not create duplicate manual entries for the same captured coding session.
",
            claude: "\
## Keito Time Tracking

Keito agent time tracking is required before billable coding work in this
repository. Before starting, verify `keito skill status --json`. If the skill
or hooks are missing, stop and tell the user to run `keito skill install --agent <claude-code|codex>`.

If `.keito/config.yml` is missing for this worktree, stop and run
`/track-time-keito` so the user can choose the Keito client, project, and task.
Do not guess project or task IDs. Do not commit `.keito/config.yml`.

The lifecycle hooks save minimized observed-turn evidence in a private local queue.
A separately supervised worker sends canonical revisions for review; elapsed activity
is not permission to bill. Verify local consent, worker and delivery with
`keito capture status --json` and `keito capture worker-status --json`.
Do not create duplicate manual entries for the same captured coding session.
",
        },
    }
}

pub(crate) fn git_repo_root() -> Result<PathBuf, AppError> {
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

fn capture_diagnostics(global: &GlobalFlags) -> (serde_json::Value, serde_json::Value) {
    let default_worker = serde_json::json!({"installed":false,"loaded":false,"running":false,"stale":false,"state":"UNKNOWN"});
    let directory = crate::capture_invocations::directory().ok();
    let Some(directory) = directory else {
        return (
            serde_json::json!({"storage_paused":true,"reason":"STORAGE_UNAVAILABLE"}),
            default_worker,
        );
    };
    let Ok(mut store) = crate::capture::Store::open(&directory) else {
        return (
            serde_json::json!({"storage_paused":true,"reason":"STORAGE_UNAVAILABLE"}),
            default_worker,
        );
    };
    let repository = std::env::current_dir()
        .ok()
        .and_then(|path| crate::capture::Repository::resolve(&path).ok());
    if let Some(repository) = &repository {
        if store.observe_repository(repository).is_err() {
            return (
                serde_json::json!({"storage_paused":true,"reason":"CAPTURE_AUTHORITY_UNAVAILABLE"}),
                default_worker,
            );
        }
    }
    let Ok(status) = store.status() else {
        return (
            serde_json::json!({"storage_paused":true,"reason":"STORAGE_UNAVAILABLE"}),
            default_worker,
        );
    };
    let mut capture =
        serde_json::to_value(status).unwrap_or_else(|_| serde_json::json!({"storage_paused":true}));
    capture["historical_delivery_verified"] = capture["delivery_verified"].clone();
    capture["delivery_verified"] = serde_json::json!(false);
    let auth = ResolvedAuth::resolve(global).ok();
    let origin = crate::config::AppConfig::load()
        .ok()
        .map(|config| crate::capture::hash(config.api_base_url().trim_end_matches('/')));
    let worker = if let Some(repository) = repository {
        capture["current_repository"] = match store.binding(&repository.key) {
            Ok(Some(binding)) => {
                let local_auth_match = auth
                    .as_ref()
                    .is_some_and(|auth| auth.workspace_id == binding.workspace_id)
                    && origin.as_deref() == Some(binding.origin_hash.as_str());
                serde_json::json!({"approved":!binding.revoked,"paused":binding.paused,"mapping_current":repository.matches_binding(&binding),
                    "workspace_id":binding.workspace_id,"owner_id":binding.owner_id,"origin_hash":binding.origin_hash,
                    "delivery_history_verified":store.binding_delivery_history(&binding.id).unwrap_or(false),
                    "authentication_binding":if local_auth_match {"UNVERIFIED_OWNER"} else {"MISMATCH"}})
            }
            _ => serde_json::json!({"approved":false,"reason":"LOCAL_APPROVAL_REQUIRED"}),
        };
        crate::capture_supervisor::Supervisor::system()
            .ok()
            .and_then(|s| s.status(&repository.key, None).ok())
            .and_then(|s| serde_json::to_value(s).ok())
            .unwrap_or(default_worker)
    } else {
        capture["current_repository"] =
            serde_json::json!({"approved":false,"reason":"REPOSITORY_UNAVAILABLE"});
        default_worker
    };
    (capture, worker)
}

#[cfg(test)]
mod bundle_tests {
    use super::*;
    use std::collections::BTreeSet;
    fn collect(root: &Path, relative: &Path, paths: &mut BTreeSet<String>) {
        for item in fs::read_dir(root.join(relative)).unwrap() {
            let item = item.unwrap();
            let next = relative.join(item.file_name());
            if item.file_name() == "__pycache__"
                || item.path().extension().is_some_and(|ext| ext == "pyc")
            {
                continue;
            }
            if item.file_type().unwrap().is_dir() {
                collect(root, &next, paths)
            } else {
                paths.insert(next.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    #[test]
    fn materialized_bundle_is_complete_and_byte_identical_to_all_shipped_sources() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/keito-time-track");
        let mut source_paths = BTreeSet::new();
        collect(&source, Path::new(""), &mut source_paths);
        let embedded: BTreeSet<_> = BUNDLED_SKILL_FILES
            .iter()
            .map(|f| f.path.to_owned())
            .collect();
        assert_eq!(
            embedded, source_paths,
            "A shipped adapter/test/manifest must never disappear from the binary bundle"
        );
        let materialized = materialize_bundled_skill().unwrap();
        for path in source_paths {
            assert_eq!(
                fs::read(source.join(&path)).unwrap(),
                fs::read(materialized.skill_root.join(&path)).unwrap(),
                "{path}"
            );
        }
    }
}
