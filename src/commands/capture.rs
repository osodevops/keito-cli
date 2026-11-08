use crate::api::client::KeitorClient;
use crate::capture::{self, Binding, Clock, Repository, Store, MAX_EVENT_BYTES};
use crate::cli::capture::{CaptureCommand, CaptureSubcommand};
use crate::cli::GlobalFlags;
use crate::config::{AppConfig, ResolvedAuth};
use crate::error::AppError;
use crate::output::OutputMode;
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn directory() -> Result<PathBuf, AppError> {
    if let Some(path) = std::env::var_os("KEITO_CAPTURE_HOME") {
        return Ok(PathBuf::from(path));
    }
    Ok(AppConfig::config_dir()?.join("capture"))
}
fn current_repo() -> Result<Repository, AppError> {
    let cwd = std::env::current_dir()
        .map_err(|_| AppError::Config("Cannot identify working directory".into()))?;
    Repository::resolve(&cwd)
}
fn print(value: Value, quiet: bool) -> Result<(), AppError> {
    if !quiet {
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|_| AppError::Config("Cannot render capture state".into()))?
        );
    }
    Ok(())
}

pub async fn run(
    cmd: CaptureCommand,
    global: &GlobalFlags,
    _mode: OutputMode,
) -> Result<(), AppError> {
    match cmd.command {
        CaptureSubcommand::Hook {
            agent,
            event,
            harness_version,
        } => {
            // A malformed/unknown hook must never block coding or echo raw input.
            let result = (|| {
                let mut bytes = Vec::new();
                std::io::stdin()
                    .take((MAX_EVENT_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|_| AppError::InvalidInput("Cannot read capture event".into()))?;
                if bytes.len() > MAX_EVENT_BYTES {
                    return Err(AppError::InvalidInput(
                        "Capture input limit exceeded".into(),
                    ));
                }
                let value: Value = serde_json::from_slice(&bytes)
                    .map_err(|_| AppError::InvalidInput("Malformed capture event".into()))?;
                let source_id = value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidInput("Missing session identity".into()))?;
                let cwd = value
                    .get("cwd")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidInput("Missing repository identity".into()))?;
                let repo = Repository::resolve(Path::new(cwd))?;
                let turn_id = value
                    .get(if agent.namespace() == "claude-code" {
                        "prompt_id"
                    } else {
                        "turn_id"
                    })
                    .and_then(Value::as_str);
                let mut store = Store::open(&directory()?)?;
                store.event(
                    &repo,
                    capture::Signal {
                        source: agent.namespace(),
                        resumed: event == crate::cli::capture::CaptureEvent::SessionStart
                            && value.get("source").and_then(Value::as_str) == Some("resume"),
                        source_id,
                        turn_id,
                        version: &harness_version,
                        event,
                        clock: &Clock::now()?,
                    },
                )
            })();
            if result.is_err() {
                eprintln!("Keito capture held this event; run keito capture status for recovery.");
            }
            Ok(())
        }
        CaptureSubcommand::Setup {
            agent,
            project,
            task,
            confirm,
            install_worker,
        } => {
            if !confirm {
                return Err(AppError::InvalidInput(
                    "Explicit --confirm is required for metadata-only capture consent".into(),
                ));
            }
            capture::identifier(&project)?;
            capture::identifier(&task)?;
            let repo = current_repo()?;
            if repo.disabled {
                return Err(AppError::Config(
                    "Repository hint disables capture; enable it before approving".into(),
                ));
            }
            let auth = ResolvedAuth::resolve(global).map_err(sanitize_error)?;
            let config = AppConfig::load().map_err(sanitize_error)?;
            let client = KeitorClient::new(&auth, &config.api_base_url())?;
            let me = client.get_me().await.map_err(sanitize_error)?;
            if me.is_personal_read_only_sync() {
                return Err(AppError::Auth(
                    "Capture requires a full-access credential".into(),
                ));
            }
            capture::identifier(&me.id)?;
            let projects = client.list_projects().await.map_err(sanitize_error)?;
            if !projects.iter().any(|p| p.id == project && p.is_active) {
                return Err(AppError::NotFound(
                    "Project unavailable; choose an active accessible project".into(),
                ));
            }
            let tasks = client
                .list_tasks_for_project(Some(&project))
                .await
                .map_err(sanitize_error)?;
            if !tasks.iter().any(|t| t.id == task && t.is_active) {
                return Err(AppError::NotFound(
                    "Task unavailable on the selected project".into(),
                ));
            }
            let mut store = Store::open(&directory()?)?;
            if install_worker && config.api_key.as_deref().is_none_or(str::is_empty) {
                return Err(AppError::Config("The supervised worker requires credentials saved by keito auth login; environment-only credentials are never copied to service settings".into()));
            }
            let approved = store.approve(Binding {
                repository_key: repo.key,
                hint_hash: repo.hint_hash,
                id: String::new(),
                version: 0,
                workspace_id: auth.workspace_id,
                owner_id: me.id,
                origin_hash: capture::hash(config.api_base_url().trim_end_matches('/')),
                project_id: project,
                task_id: task,
                agents: agent
                    .into_iter()
                    .map(|a| a.namespace().to_owned())
                    .collect(),
                paused: false,
                revoked: false,
            })?;
            let worker = if install_worker {
                Some(install_worker_service(
                    &approved,
                    &origin_for_worker(&config),
                    &directory()?,
                )?)
            } else {
                None
            };
            print(
                json!({"status":"approved","worker":worker,"policy":"METADATA_ONLY_OBSERVED_TURNS_V1","binding":approved,
                "notice":"Only minimized timing metadata leaves this device. Captures require review before charging. Start capture worker for automatic delivery.","usage":"UNAVAILABLE"}),
                global.quiet,
            )
        }
        CaptureSubcommand::Status => {
            let store = Store::open(&directory()?)?;
            let mut value = serde_json::to_value(store.status()?)
                .map_err(|_| AppError::Config("Cannot render capture status".into()))?;
            value["current_repository"] = match current_repo() {
                Ok(repo) => match store.binding(&repo.key)? {
                    Some(b) => {
                        json!({"approved":!b.revoked,"paused":b.paused,"mapping_current":b.hint_hash==repo.hint_hash,"binding_id":b.id})
                    }
                    None => json!({"approved":false,"reason":"LOCAL_APPROVAL_REQUIRED"}),
                },
                Err(_) => json!({"approved":false,"reason":"REPOSITORY_UNAVAILABLE"}),
            };
            print(value, global.quiet)
        }
        CaptureSubcommand::Preview { session } => {
            let store = Store::open(&directory()?)?;
            print(json!({"sessions":store.preview(&session)?}), global.quiet)
        }
        CaptureSubcommand::Sync => {
            let result = sync(global).await?;
            print(result, global.quiet)
        }
        CaptureSubcommand::Worker { interval_seconds } => {
            // Foreground supervisor: launchd/systemd/the operator owns lifetime. A
            // detached hook child is never the sole delivery/durability mechanism.
            loop {
                let _ = sync(global).await;
                tokio::select! { _ = tokio::signal::ctrl_c() => break, _ = tokio::time::sleep(Duration::from_secs(interval_seconds)) => {} }
            }
            Ok(())
        }
        CaptureSubcommand::WorkerStatus => {
            let value = match (
                current_repo(),
                crate::capture_supervisor::Supervisor::system(),
            ) {
                (Ok(repo), Ok(supervisor)) => {
                    let executable = std::env::current_exe()
                        .map_err(|_| AppError::Config("Cannot identify worker runtime".into()))?;
                    let cwd = std::env::current_dir().map_err(|_| {
                        AppError::Config("Cannot identify worker repository".into())
                    })?;
                    let store = Store::open(&directory()?)?;
                    let binding = store.binding(&repo.key)?;
                    let config = AppConfig::load().map_err(sanitize_error)?;
                    let origin = config.api_base_url();
                    let capture_home = directory()?;
                    let expected =
                        binding
                            .as_ref()
                            .map(|binding| crate::capture_supervisor::WorkerConfig {
                                repository_key: &repo.key,
                                repository_root: &cwd,
                                executable: &executable,
                                capture_home: &capture_home,
                                workspace_id: &binding.workspace_id,
                                api_origin: &origin,
                            });
                    serde_json::to_value(supervisor.status(&repo.key, expected.as_ref())?)
                        .map_err(|_| AppError::Config("Cannot inspect worker service".into()))?
                }
                _ => {
                    json!({"supported":cfg!(target_os="macos"),"installed":false,"running":false,"stale":false})
                }
            };
            print(value, global.quiet)
        }
        CaptureSubcommand::WorkerUninstall { .. } => {
            let repo = current_repo()?;
            let supervisor = crate::capture_supervisor::Supervisor::system()?;
            print(
                serde_json::to_value(supervisor.uninstall(&repo.key)?)
                    .map_err(|_| AppError::Config("Cannot inspect worker service".into()))?,
                global.quiet,
            )
        }
        CaptureSubcommand::Pause { .. }
        | CaptureSubcommand::Resume { .. }
        | CaptureSubcommand::Disable { .. } => {
            let action = match cmd.command {
                CaptureSubcommand::Pause { .. } => "pause",
                CaptureSubcommand::Resume { .. } => "resume",
                _ => "disable",
            };
            let repo = current_repo()?;
            let mut store = Store::open(&directory()?)?;
            store.control(&repo, action, &Clock::now()?)?;
            print(
                json!({"status":action,"binding":store.binding(&repo.key)?}),
                global.quiet,
            )
        }
        CaptureSubcommand::DisableAll => {
            let mut store = Store::open(&directory()?)?;
            store.disable_all()?;
            print(
                json!({"status":"disabled_all","notice":"Unsent evidence retained; no further uploads are authorized"}),
                global.quiet,
            )
        }
    }
}

async fn sync(global: &GlobalFlags) -> Result<Value, AppError> {
    let auth = ResolvedAuth::resolve(global).map_err(sanitize_error)?;
    let config = AppConfig::load().map_err(sanitize_error)?;
    let origin = config.api_base_url();
    let client = KeitorClient::new(&auth, &origin)?;
    let me = client.get_me().await.map_err(sanitize_error)?;
    let mut store = Store::open(&directory()?)?;
    let mut accepted = 0;
    let mut held = 0;
    let mut retryable = 0;
    // Worker is deliberately repository-scoped: never scan arbitrary folders or
    // trust a copied configuration file when rechecking a pending upload.
    let repo = current_repo()?;
    let Some(binding) = store.binding(&repo.key)? else {
        return Ok(json!({"accepted":0,"held":0,"reason":"LOCAL_APPROVAL_REQUIRED"}));
    };
    if repo.disabled
        || binding.paused
        || binding.revoked
        || binding.hint_hash != repo.hint_hash
        || binding.workspace_id != auth.workspace_id
        || binding.owner_id != me.id
        || binding.origin_hash != capture::hash(origin.trim_end_matches('/'))
    {
        return Ok(json!({"accepted":0,"held":1,"reason":"BINDING_REAPPROVAL_REQUIRED"}));
    }
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| AppError::Network("Cannot initialize capture transport".into()))?;
    // A sync invocation has bounded work; queued revisions remain durable.
    for _ in 0..100 {
        let now = UtcClock::seconds();
        let Some(row) = store.claim(&binding, now)? else {
            break;
        };
        // Current consent was checked atomically at claim. The server rechecks
        // authority before accepting every revision and replay.
        let response = http
            .post(format!(
                "{}/api/v2/agent-sessions",
                origin.trim_end_matches('/')
            ))
            .header("Authorization", format!("Bearer {}", auth.api_key))
            .header("Keito-Account-Id", &auth.workspace_id)
            .header("Content-Type", "application/json")
            .header("Idempotency-Key", &row.hash)
            .header("X-Keito-Client", "cli")
            .header("X-Keito-Client-Version", env!("CARGO_PKG_VERSION"))
            .body(row.body.clone())
            .send()
            .await;
        match response {
            Err(_) => {
                store.finish(&row, "pending", "NETWORK_UNAVAILABLE", now, None)?;
                retryable += 1;
                break;
            }
            Ok(response) => {
                let status = response.status().as_u16();
                let retry_after = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|h| h.to_str().ok())
                    .and_then(|h| h.parse::<i64>().ok());
                if (200..300).contains(&status) {
                    // Read bounded acknowledgements; never persist server prose.
                    if response.content_length().is_some_and(|n| n > 64 * 1024) {
                        store.finish(&row, "pending", "INVALID_ACKNOWLEDGEMENT", now, None)?;
                        retryable += 1;
                        break;
                    }
                    let expected: Value = serde_json::from_str(&row.body)
                        .map_err(|_| AppError::Config("Invalid frozen capture evidence".into()))?;
                    let ack = bounded_ack(response).await;
                    if ack.as_ref().is_some_and(|a| {
                        a["agent_session"]["revision"] == expected["revision"]
                            && a["agent_session"]["id"].is_string()
                    }) {
                        store.finish(&row, "acknowledged", "ACCEPTED", now, None)?;
                        accepted += 1;
                    } else {
                        store.finish(&row, "pending", "INVALID_ACKNOWLEDGEMENT", now, None)?;
                        retryable += 1;
                        break;
                    }
                } else if status == 429 || status >= 500 {
                    store.finish(
                        &row,
                        "pending",
                        if status == 429 {
                            "RATE_LIMITED"
                        } else {
                            "SERVER_UNAVAILABLE"
                        },
                        now,
                        retry_after,
                    )?;
                    retryable += 1;
                    break;
                } else {
                    let reason = match status {
                        401 => "NEEDS_AUTH",
                        403 => "FORBIDDEN_OR_DISABLED",
                        404 => "MAPPING_OR_API_UNAVAILABLE",
                        409 => "REVISION_CONFLICT",
                        413 => "PAYLOAD_LIMIT",
                        _ => "REQUEST_REJECTED",
                    };
                    // Auth/feature disable are recoverable with the same payload.
                    let state = if status == 401 || status == 403 || status == 404 {
                        "pending"
                    } else {
                        "held"
                    };
                    store.finish(&row, state, reason, now, Some(60))?;
                    held += 1;
                    break;
                }
            }
        }
    }
    Ok(
        json!({"accepted":accepted,"held":held,"retryable":retryable,"pending":store.status()?.pending}),
    )
}
struct UtcClock;
impl UtcClock {
    fn seconds() -> i64 {
        chrono::Utc::now().timestamp()
    }
}

async fn bounded_ack(mut response: reqwest::Response) -> Option<Value> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len().checked_add(chunk.len())? > 64 * 1024 {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).ok()
}

fn origin_for_worker(config: &AppConfig) -> String {
    config.api_base_url()
}
fn install_worker_service(
    binding: &Binding,
    origin: &str,
    capture_home: &Path,
) -> Result<crate::capture_supervisor::WorkerStatus, AppError> {
    let supervisor = crate::capture_supervisor::Supervisor::system()?;
    let cwd = std::env::current_dir()
        .map_err(|_| AppError::Config("Cannot identify worker repository".into()))?;
    let executable = std::env::current_exe()
        .map_err(|_| AppError::Config("Cannot identify CLI runtime".into()))?;
    let config = crate::capture_supervisor::WorkerConfig {
        repository_key: &binding.repository_key,
        repository_root: &cwd,
        executable: &executable,
        capture_home,
        workspace_id: &binding.workspace_id,
        api_origin: origin,
    };
    supervisor.install(&config)
}

fn sanitize_error(error: AppError) -> AppError {
    match error {
        AppError::Auth(_) => AppError::Auth(
            "Capture preflight needs valid current credentials and workspace permission".into(),
        ),
        AppError::RateLimited {
            retry_after_seconds,
        } => AppError::RateLimited {
            retry_after_seconds,
        },
        AppError::Network(_) => AppError::Network(
            "Capture preflight network unavailable; frozen evidence retained".into(),
        ),
        AppError::NotFound(_) => AppError::NotFound(
            "Capture mapping unavailable; review the selected project and task".into(),
        ),
        AppError::Conflict(_) => {
            AppError::Conflict("Capture preflight conflicted; review current authority".into())
        }
        AppError::InvalidInput(_) => {
            AppError::InvalidInput("Capture preflight rejected the request".into())
        }
        AppError::Config(_)=>AppError::Config("Capture configuration unavailable; repair local settings without printing their contents".into()),
        _ => AppError::ServerError(
            "Capture preflight unavailable; no response content retained".into(),
        ),
    }
}
