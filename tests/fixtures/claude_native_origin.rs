#[cfg(unix)]
#[path = "capture_native_origin.rs"]
mod origin;
fn main() {
    #[cfg(unix)]
    std::process::exit(origin::run("2.1.319"));
    #[cfg(not(unix))]
    std::process::exit(1);
}
