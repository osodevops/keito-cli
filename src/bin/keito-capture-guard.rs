#[path = "../native_capture_guard.rs"]
mod native_capture_guard;
#[cfg(unix)]
#[path = "../process_ownership/mod.rs"]
#[allow(dead_code)] // Core-only APIs share this source; the guard links only its roles.
pub mod process_ownership;
fn main() {
    #[cfg(unix)]
    {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if args
            .first()
            .is_some_and(|a| a == process_ownership::image::KEEPER_ROLE)
        {
            let fields = &args[1..];
            let result = (|| {
                if !matches!(fields.len(), 6 | 7) {
                    return Err(());
                }
                let control = fields[0]
                    .to_str()
                    .and_then(|s| s.parse::<i32>().ok())
                    .filter(|fd| (4..=1024).contains(fd))
                    .ok_or(())?;
                process_ownership::image::bootstrap_keeper_image(control).map_err(|_| ())?;
                process_ownership::group::keeper_main(fields.iter()).map_err(|_| ())
            })();
            if result.is_err() {
                std::process::exit(1);
            }
            return;
        }
    }
    #[cfg(unix)]
    {
        // Every installed role is owned on this entry thread before it can
        // create a Child. Private keeper startup remains its closed role above.
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        let role = args.first().and_then(|a| a.to_str());
        let dispatch = role == Some(process_ownership::image::DISPATCH_ROLE);
        let guardian = role == Some(process_ownership::image::GUARDIAN_ROLE);
        if guardian && process_ownership::image::bootstrap_role_image(4).is_err() { return; }
        let fields = if dispatch || guardian { &args[1..] } else { &args[..] };
        let duration = if dispatch { 675_000_000 } else { 600_000_000 };
        let mut scope = match process_ownership::scope::Scope::entry_from_environment(duration) {
            Ok(Some(scope)) => scope,
            Ok(None) => {
                let Ok(at) = process_ownership::scope::monotonic_now() else { return; };
                process_ownership::scope::Scope::with_owner(Default::default(), at.saturating_add(duration))
            }
            Err(_) => return,
        };
        let result = if dispatch { dispatch_guard(fields, &mut scope) } else { native_capture_guard::run(fields, &mut scope) };
        let clean = scope.finish().clean && result.is_ok();
        scope.publish_terminal(clean);
        // Installed hooks remain silent/fail-open; a failed/absent ownership
        // terminal and retained durable marker independently forbid time proof.
    }
}

#[cfg(unix)]
fn dispatch_guard(args: &[std::ffi::OsString], scope: &mut process_ownership::scope::Scope) -> Result<(), ()> {
    use std::{os::fd::AsRawFd, process::Stdio, time::Duration};
    if args.len() != 8 { return Err(()); }
    let image = process_ownership::image::VerifiedKeeperImage::running_guard().map_err(|_| ())?;
    let (mut command, descriptor) = image.guardian_command(4).map_err(|_| ())?;
    command.args(args)
        .env(process_ownership::image::VerifiedKeeperImage::image_env(), descriptor.as_raw_fd().to_string())
        .stdin(Stdio::inherit()).stdout(Stdio::null()).stderr(Stdio::null());
    let child = scope.start_cooperative(command).map_err(|_| ())?;
    drop(descriptor);
    loop {
        scope.poll();
        if scope.slot_complete(&child) {
            return scope.slot_status(&child).filter(|s| s.success()).map(|_| ()).ok_or(());
        }
        if scope.failed() { return Err(()); }
        std::thread::sleep(Duration::from_millis(1));
    }
}
