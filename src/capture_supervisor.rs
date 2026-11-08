//! macOS operational service configuration. Paths remain device-local and are
//! never part of capture evidence, queue records or transport.
use crate::capture::hash;
use crate::error::AppError;
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const MARKER: &str = "METADATA_ONLY_OBSERVED_TURNS_V1";
fn failure() -> AppError {
    AppError::Config("Capture worker service operation failed; prior settings retained where possible. Inspect worker-status before retrying.".into())
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
#[derive(Clone)]
pub struct Supervisor {
    pub home: PathBuf,
    pub launchctl: PathBuf,
    pub uid: u32,
}
#[derive(Debug, Serialize)]
pub struct WorkerStatus {
    pub supported: bool,
    pub installed: bool,
    pub running: bool,
    pub stale: bool,
    pub policy: &'static str,
}
pub struct WorkerConfig<'a> {
    pub repository_key: &'a str,
    pub repository_root: &'a Path,
    pub executable: &'a Path,
    pub capture_home: &'a Path,
    pub workspace_id: &'a str,
    pub api_origin: &'a str,
}
impl Supervisor {
    pub fn system() -> Result<Self, AppError> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                home: dirs::home_dir().ok_or_else(failure)?,
                launchctl: PathBuf::from("/bin/launchctl"),
                uid: unsafe { libc::getuid() },
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(AppError::Config("Automatic worker supervision is currently qualified for macOS only; run capture worker under your own supervisor".into()))
        }
    }
    fn label(key: &str) -> String {
        format!("com.keito.capture.{}", hash(key))
    }
    fn file(&self, key: &str) -> PathBuf {
        self.home
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", Self::label(key)))
    }
    fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }
    fn service(&self, key: &str) -> String {
        format!("{}/{}", self.domain(), Self::label(key))
    }
    pub fn render(&self, config: &WorkerConfig<'_>) -> Result<String, AppError> {
        crate::capture::identifier(config.workspace_id)?;
        let url = reqwest::Url::parse(config.api_origin).map_err(|_| failure())?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || (url.scheme() != "https"
                && !(url.scheme() == "http"
                    && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        {
            return Err(failure());
        }
        let binary = fs::read(config.executable).map_err(|_| failure())?;
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{label}</string>
<key>KeitoCapturePolicy</key><string>{policy}</string>
<key>KeitoBinarySha256</key><string>{binary_hash}</string>
<key>ProgramArguments</key><array><string>{executable}</string><string>--workspace</string><string>{workspace}</string><string>capture</string><string>worker</string><string>--interval-seconds</string><string>5</string></array>
<key>WorkingDirectory</key><string>{repository}</string>
<key>EnvironmentVariables</key><dict><key>KEITO_CAPTURE_HOME</key><string>{capture_home}</string><key>KEITO_API_URL</key><string>{origin}</string></dict>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
<key>ProcessType</key><string>Background</string><key>ThrottleInterval</key><integer>10</integer>
<key>StandardOutPath</key><string>/dev/null</string><key>StandardErrorPath</key><string>/dev/null</string>
</dict></plist>
"#,
            label = Self::label(config.repository_key),
            policy = MARKER,
            binary_hash = hash(binary),
            executable = xml(&config.executable.to_string_lossy()),
            workspace = xml(config.workspace_id),
            repository = xml(&config.repository_root.to_string_lossy()),
            capture_home = xml(&config.capture_home.to_string_lossy()),
            origin = xml(config.api_origin)
        ))
    }
    fn owned(contents: &str, key: &str) -> bool {
        contents.contains(&format!(
            "<key>Label</key><string>{}</string>",
            Self::label(key)
        )) && contents.contains(&format!(
            "<key>KeitoCapturePolicy</key><string>{MARKER}</string>"
        ))
    }
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, AppError> {
        let file = self.file(key);
        if !file.exists() {
            return Ok(None);
        }
        let metadata = fs::symlink_metadata(&file).map_err(|_| failure())?;
        if metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 {
            return Err(failure());
        }
        let bytes = fs::read(&file).map_err(|_| failure())?;
        if !Self::owned(&String::from_utf8_lossy(&bytes), key) {
            return Err(AppError::Config(
                "Refusing to replace an unrecognized launch agent; no settings changed".into(),
            ));
        }
        Ok(Some(bytes))
    }
    fn atomic_write(&self, key: &str, bytes: &[u8]) -> Result<(), AppError> {
        let file = self.file(key);
        let directory = file.parent().ok_or_else(failure)?;
        if directory.exists()
            && fs::symlink_metadata(directory)
                .map_err(|_| failure())?
                .file_type()
                .is_symlink()
        {
            return Err(failure());
        }
        let created = !directory.exists();
        fs::create_dir_all(directory).map_err(|_| failure())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if created {
                fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                    .map_err(|_| failure())?;
            }
        }
        #[cfg(not(unix))]
        let _ = created;
        let temporary = file.with_extension(format!("plist.{}.tmp", std::process::id()));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut output = options.open(&temporary).map_err(|_| failure())?;
            output.write_all(bytes).map_err(|_| failure())?;
            output.sync_all().map_err(|_| failure())?;
            fs::rename(&temporary, &file).map_err(|_| failure())?;
            Ok(())
        })();
        if temporary.exists() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
    pub fn status(
        &self,
        key: &str,
        config: Option<&WorkerConfig<'_>>,
    ) -> Result<WorkerStatus, AppError> {
        let contents = self.read(key)?;
        let installed = contents.is_some();
        let running = installed
            && Command::new(&self.launchctl)
                .args(["print", &self.service(key)])
                .output()
                .is_ok_and(|o| {
                    o.status.success()
                        && String::from_utf8_lossy(&o.stdout).contains("state = running")
                        && String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .any(|line| line.trim().starts_with("pid = "))
                });
        let stale = if let (Some(existing), Some(config)) = (&contents, config) {
            *existing != self.render(config)?.as_bytes()
        } else if let Some(existing) = &contents {
            let executable = std::env::current_exe().map_err(|_| failure())?;
            let current = hash(fs::read(executable).map_err(|_| failure())?);
            !String::from_utf8_lossy(existing).contains(&format!(
                "<key>KeitoBinarySha256</key><string>{current}</string>"
            ))
        } else {
            false
        };
        Ok(WorkerStatus {
            supported: true,
            installed,
            running,
            stale,
            policy: MARKER,
        })
    }
    pub fn install(&self, config: &WorkerConfig<'_>) -> Result<WorkerStatus, AppError> {
        let key = config.repository_key;
        let desired = self.render(config)?;
        let prior = self.read(key)?;
        if prior.as_deref() == Some(desired.as_bytes()) && self.status(key, Some(config))?.running {
            return self.status(key, Some(config));
        }
        self.atomic_write(key, desired.as_bytes())?;
        let _ = Command::new(&self.launchctl)
            .args(["bootout", &self.service(key)])
            .output();
        let result = Command::new(&self.launchctl)
            .arg("bootstrap")
            .arg(self.domain())
            .arg(self.file(key))
            .output();
        if !result.is_ok_and(|o| o.status.success()) {
            if let Some(prior) = prior {
                self.atomic_write(key, &prior)?;
                let _ = Command::new(&self.launchctl)
                    .arg("bootstrap")
                    .arg(self.domain())
                    .arg(self.file(key))
                    .output();
            } else {
                let _ = fs::remove_file(self.file(key));
            }
            return Err(failure());
        }
        self.status(key, Some(config))
    }
    pub fn uninstall(&self, key: &str) -> Result<WorkerStatus, AppError> {
        if self.read(key)?.is_none() {
            return self.status(key, None);
        }
        let result = Command::new(&self.launchctl)
            .args(["bootout", &self.service(key)])
            .output();
        if !result.is_ok_and(|o| o.status.success()) && self.status(key, None)?.running {
            return Err(failure());
        }
        fs::remove_file(self.file(key)).map_err(|_| failure())?;
        self.status(key, None)
    }
}
