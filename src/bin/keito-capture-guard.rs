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
        let fields = if dispatch {
            &args[1..]
        } else {
            &args[..]
        };
        let duration = 600_000_000;
        let mut scope = match process_ownership::scope::Scope::entry_from_environment(duration) {
            Ok(Some(scope)) => scope,
            Ok(None) => {
                let Ok(at) = process_ownership::scope::monotonic_now() else {
                    return;
                };
                match process_ownership::scope::Scope::local_entry(at.saturating_add(duration)) {
                    Ok(scope) => scope,
                    Err(_) => return,
                }
            }
            Err(_) => return,
        };
        // Dispatch and guardian share one durable prelaunch entry. No image
        // hash/verification or child admission precedes marker publication.
        let result = native_capture_guard::run(fields, &mut scope);
        let clean = scope.finish().clean && result.is_ok();
        scope.publish_terminal(clean);
        // Installed hooks remain silent/fail-open; a failed/absent ownership
        // terminal and retained durable marker independently forbid time proof.
    }
}
