//! `pacman -U` / `-R` / `-Q` / `-Qo` wrappers.

use crate::error::{Error, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// Owner of an installed path via `pacman -Qo`.
///
/// Relative paths are prefixed with `/` so `usr/bin/hello` → `/usr/bin/hello`.
/// Callers must pass the install path, never a workdir `payload/` path.
pub fn owned_by(abs_path: &Path) -> Result<Option<String>> {
    let path = Path::new("/").join(abs_path);
    if let Some(hook) = crate::hooks::owned_by_hook() {
        return hook(&path);
    }
    pacman_qo(&path)
}

fn pacman_qo(path: &Path) -> Result<Option<String>> {
    let out = Command::new("pacman")
        .args(["-Qo", "--"])
        .arg(path)
        .output()?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(parse_owned_by(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_owned_by(text: &str) -> Option<String> {
    // "/usr/bin/ls is owned by coreutils 9.5-1"
    let rest = text
        .lines()
        .find_map(|l| l.split_once(" is owned by ").map(|(_, r)| r))?;
    rest.split_whitespace().next().map(str::to_string)
}

/// `pacman -U` (streamed). `--noconfirm` only when requested.
pub fn upgrade(pkg_path: &Path, noconfirm: bool) -> Result<()> {
    run_pacman(noconfirm, &["-U"], &[pkg_path.as_os_str()])
}

/// `pacman -R` (streamed, not `-Rns`). `--noconfirm` only when requested.
pub fn remove(pkg: &str, noconfirm: bool) -> Result<()> {
    run_pacman(noconfirm, &["-R"], &[std::ffi::OsStr::new(pkg)])
}

fn run_pacman(noconfirm: bool, verb: &[&str], tail: &[&std::ffi::OsStr]) -> Result<()> {
    let mut cmd = Command::new("pacman");
    cmd.args(verb);
    if noconfirm {
        cmd.arg("--noconfirm");
    }
    cmd.arg("--");
    cmd.args(tail);
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    let status = cmd.status()?;
    if !status.success() {
        return Err(Error::msg(format!("pacman {} failed", verb.join(" "))));
    }
    Ok(())
}

/// `pacman -Q`; `Some(version)` if installed.
pub fn query(pkg: &str) -> Result<Option<String>> {
    let out = Command::new("pacman")
        .args(["-Q", "--"])
        .arg(pkg)
        .output()?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .nth(1)
        .map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_by_prefixes_slash() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SAW_ABS: AtomicBool = AtomicBool::new(false);
        fn fake(p: &Path) -> crate::error::Result<Option<String>> {
            SAW_ABS.store(p.is_absolute() && p.starts_with("/usr"), Ordering::SeqCst);
            assert!(p == Path::new("/usr/bin/hello"), "{}", p.display());
            Ok(Some("other".into()))
        }
        let _g = crate::hooks::set_owned_by(fake);
        let got = owned_by(Path::new("usr/bin/hello")).unwrap();
        assert_eq!(got.as_deref(), Some("other"));
        assert!(SAW_ABS.load(Ordering::SeqCst));
    }
}
