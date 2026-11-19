#[cfg(unix)]
#[path = "capture_native_origin.rs"]
mod origin;
fn main() {
    #[cfg(unix)]
    std::process::exit(origin::run("0.167.0"));
    #[cfg(not(unix))]
    std::process::exit(1);
}
