#[path = "../native_capture_guard.rs"]
mod native_capture_guard;
#[cfg(unix)]
#[path = "../process_ownership/mod.rs"]
#[allow(dead_code)] // Core-only APIs share this source; the guard links only its roles.
pub mod process_ownership;
fn main() {
    #[cfg(unix)]
    {
        let args:Vec<_>=std::env::args_os().skip(1).collect();
        if args.first().is_some_and(|a| a==process_ownership::image::KEEPER_ROLE) {
            let fields=&args[1..];
            let result=(|| {
                if !matches!(fields.len(),6|7) {return Err(());}
                let control=fields[0].to_str().and_then(|s|s.parse::<i32>().ok()).filter(|fd|(4..=1024).contains(fd)).ok_or(())?;
                process_ownership::image::bootstrap_keeper_image(control).map_err(|_|())?;
                process_ownership::group::keeper_main(fields.iter()).map_err(|_|())
            })();
            if result.is_err() {std::process::exit(1);}
            return;
        }
    }
    native_capture_guard::main();
}
