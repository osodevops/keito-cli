//! Development-only preflight. Call before constructing any mutation command.
use std::path::{Path, PathBuf};
pub struct CheckedEnvironment {
    pub home: PathBuf,
    pub capture: PathBuf,
    pub core: PathBuf,
}
pub fn checked(
    run_root: &Path,
    case_root: &Path,
    home: Option<&Path>,
    capture: Option<&Path>,
    core: Option<&Path>,
) -> Result<CheckedEnvironment, &'static str> {
    let run_root = run_root
        .canonicalize()
        .map_err(|_| "run-owned root unavailable")?;
    let case_root = case_root
        .canonicalize()
        .map_err(|_| "case root unavailable")?;
    if !case_root.starts_with(&run_root) {
        return Err("foreign case refused before mutation");
    }
    let home = home.ok_or("missing explicit isolated HOME")?;
    let capture = capture.ok_or("missing explicit isolated capture root")?;
    let core = core.ok_or("missing explicit run-owned core")?;
    if home != case_root.join("home")
        || capture != home.join("capture")
        || !core.is_absolute()
        || !core.starts_with(&run_root)
    {
        return Err("foreign environment refused before mutation");
    }
    if home.canonicalize().ok().as_deref() != Some(home)
        || capture.canonicalize().ok().as_deref() != Some(capture)
    {
        return Err("aliased or missing environment refused before mutation");
    }
    for name in [".codex", ".claude"] {
        let path = home.join(name);
        if (path.exists() || path.is_symlink())
            && (path.is_symlink()
                || !path.is_dir()
                || path.canonicalize().ok().as_deref() != Some(path.as_path()))
        {
            return Err("harness profile escape refused before mutation");
        }
    }
    let parent = core.parent().ok_or("missing run-owned core parent")?;
    if parent.canonicalize().ok().as_deref() != Some(parent)
        || core.is_symlink()
        || (core.exists() && core.canonicalize().ok().as_deref() != Some(core))
    {
        return Err("core escape refused before mutation");
    }
    Ok(CheckedEnvironment {
        home: home.to_owned(),
        capture: capture.to_owned(),
        core: core.to_owned(),
    })
}
