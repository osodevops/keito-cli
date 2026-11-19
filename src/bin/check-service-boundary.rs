//! Development-only, single inherited macOS execution/network boundary.
#[cfg(target_os = "macos")]
#[path = "../../tests/support/mutation_environment.rs"]
mod isolation;
#[cfg(target_os = "macos")]
mod boundary {
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::{
        collections::BTreeSet,
        fs,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        os::unix::fs::{symlink, MetadataExt, PermissionsExt},
        path::{Path, PathBuf},
        process::Command,
        time::Duration,
    };
    fn hash(path: &Path) -> String {
        Sha256::digest(fs::read(path).unwrap())
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect()
    }
    fn root(path: &Path) -> PathBuf {
        let root = path.canonicalize().unwrap();
        let m = fs::metadata(&root).unwrap();
        assert!(m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o777 == 0o700);
        root
    }
    fn command(program: &Path, base: &Path, core: &Path) -> Command {
        let home = base.join("home");
        let capture = home.join("capture");
        let c =
            super::isolation::checked(base, base, Some(&home), Some(&capture), Some(core)).unwrap();
        assert_eq!(c.core, core);
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .current_dir(base)
            .env("HOME", &c.home)
            .env("CODEX_HOME", c.home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", c.home.join(".claude"))
            .env("KEITO_CAPTURE_HOME", &c.capture)
            .env("XDG_CONFIG_HOME", c.home.join("config"))
            .env("APPDATA", c.home.join("AppData"))
            .env("PATH", base.join("empty-bin"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        cmd
    }
    pub fn run() {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        assert_eq!(args.len(), 2);
        let mode = args[0].to_str().unwrap();
        let base = root(Path::new(&args[1]));
        if mode == "marker" {
            fs::write(base.join("native.marker"), b"owned-marker").unwrap();
            return;
        }
        if mode == "loopback" {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let worker = std::thread::spawn(move || {
                let (mut peer, _) = listener.accept().unwrap();
                peer.write_all(b"ok").unwrap();
            });
            let mut client = TcpStream::connect_timeout(&address, Duration::from_secs(1)).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut bytes = [0; 2];
            client.read_exact(&mut bytes).unwrap();
            assert_eq!(&bytes, b"ok");
            worker.join().unwrap();
            fs::write(base.join("loopback.marker"), b"loopback-only").unwrap();
            return;
        }
        if mode == "prepare" {
            fs::create_dir(base.join("home")).unwrap();
            fs::create_dir(base.join("home/capture")).unwrap();
            fs::set_permissions(base.join("home/capture"), fs::Permissions::from_mode(0o700))
                .unwrap();
            for name in ["blocked-native-marker", "allowed-native-marker"] {
                fs::copy(std::env::current_exe().unwrap(), base.join(name)).unwrap();
                fs::set_permissions(base.join(name), fs::Permissions::from_mode(0o700)).unwrap();
            }
            fs::create_dir(base.join("allowed")).unwrap();
            fs::set_permissions(base.join("allowed"), fs::Permissions::from_mode(0o700)).unwrap();
            symlink(
                base.join("blocked-native-marker"),
                base.join("blocked-alias"),
            )
            .unwrap();
            fs::copy(std::env::current_exe().unwrap(), base.join("owned-checker")).unwrap();
            fs::set_permissions(
                base.join("owned-checker"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            let mut denied = BTreeSet::new();
            for path in [
                PathBuf::from("/bin/launchctl"),
                PathBuf::from("/usr/bin/launchctl"),
                base.join("blocked-native-marker"),
                base.join("blocked-alias"),
            ] {
                denied.insert(path.clone());
                if let Ok(p) = path.canonicalize() {
                    denied.insert(p);
                }
            }
            let literals = denied
                .iter()
                .map(|p| {
                    format!(
                        "(literal \"{}\")",
                        p.to_string_lossy()
                            .replace('\\', "\\\\")
                            .replace('"', "\\\"")
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            let policy=format!("(version 1)\n(allow default)\n(deny process-exec {literals})\n(deny network*)\n(allow network-bind (local ip \"localhost:*\"))\n(allow network-inbound (local ip \"localhost:*\"))\n(allow network-outbound (remote ip \"localhost:*\"))\n");
            fs::write(base.join("boundary.sb"), policy).unwrap();
            fs::set_permissions(base.join("boundary.sb"), fs::Permissions::from_mode(0o600))
                .unwrap();
            return;
        }
        assert_eq!(mode, "verify");
        let blocked = base.join("blocked-native-marker");
        let allowed = base.join("allowed-native-marker");
        let marker = base.join("native.marker");
        let denied = command(&blocked, &base, &blocked)
            .arg("marker")
            .arg(&base)
            .output();
        let alias = base.join("blocked-alias");
        assert_eq!(alias.canonicalize().unwrap(), blocked);
        let denied_alias = command(&alias, &base, &blocked)
            .arg("marker")
            .arg(&base)
            .output();
        let allowed_marker = base.join("allowed/native.marker");
        let control = command(&allowed, &base, &allowed)
            .arg("marker")
            .arg(base.join("allowed"))
            .output()
            .unwrap();
        let checker = base.join("owned-checker");
        let loopback = command(&checker, &base, &checker)
            .arg("loopback")
            .arg(&base)
            .output()
            .unwrap();
        let permission_denied = |result: &std::io::Result<std::process::Output>| {
            result
                .as_ref()
                .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
        };
        let passed = permission_denied(&denied)
            && permission_denied(&denied_alias)
            && !marker.exists()
            && control.status.success()
            && allowed_marker.is_file()
            && loopback.status.success()
            && base.join("loopback.marker").is_file();
        let result = json!({"schema":1,"passed":passed,"denied_errno":denied.as_ref().err().and_then(std::io::Error::raw_os_error),"alias_errno":denied_alias.as_ref().err().and_then(std::io::Error::raw_os_error),"control_status":control.status.code(),"loopback_status":loopback.status.code(),"actual_service_utility_executed":false,"actual_service_negative_probe":false,"policy_sha256":hash(&base.join("boundary.sb")),"checker_sha256":hash(&checker),"blocked_sha256":hash(&blocked),"checker_source_sha256":Sha256::digest(include_bytes!("check-service-boundary.rs")).iter().map(|v|format!("{v:02x}")).collect::<String>(),"helper_source_sha256":Sha256::digest(include_bytes!("../../tests/support/service_boundary.rs")).iter().map(|v|format!("{v:02x}")).collect::<String>()});
        fs::write(
            base.join("proof-result.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
        assert!(
            passed,
            "single inherited service/network policy proof failed; inspect fixed result codes"
        );
        fs::write(
            base.join("service-boundary-receipt.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
        fs::set_permissions(
            base.join("service-boundary-receipt.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
    }
}
fn main() {
    #[cfg(target_os = "macos")]
    boundary::run();
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("The macOS test boundary is unavailable on this platform");
        std::process::exit(1);
    }
}
