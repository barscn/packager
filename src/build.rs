//! Run `makepkg` in a workdir (`-f` only, never `-s`).
//!
//! Root is only for `pacman -U`. When euid is 0, `makepkg` runs as `SUDO_USER`
//! (not as root, never `--asroot`). Root without `SUDO_USER` is an error.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `makepkg -f` in `workdir`; returns the built `*.pkg.tar.*`.
pub fn makepkg(workdir: &Path) -> Result<PathBuf> {
    let args = ["-f"];
    if args.iter().any(|a| is_sync_flag(a)) {
        return Err(Error::msg("makepkg must not pass -s"));
    }

    let status = match makepkg_run_as()? {
        None => Command::new("makepkg")
            .args(args)
            .current_dir(workdir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| Error::msg(format!("makepkg: {e}")))?,
        Some(user) => {
            // Workdir/payload/PKGBUILD may have been written as root under `sudo`.
            chown_tree(workdir, &user)?;
            Command::new("runuser")
                .arg("-u")
                .arg(&user)
                .arg("--")
                .arg("makepkg")
                .args(args)
                .current_dir(workdir)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
                .map_err(|e| Error::msg(format!("makepkg (as {user}): {e}")))?
        }
    };

    if !status.success() {
        return Err(Error::msg("makepkg failed"));
    }
    find_pkg(workdir)
}

/// Who should run makepkg: `None` = current process; `Some(user)` = drop to that user.
/// Root without non-empty `SUDO_USER` → error (convert as a normal user).
fn makepkg_run_as() -> Result<Option<String>> {
    if !euid_is_root() {
        return Ok(None);
    }
    match std::env::var("SUDO_USER") {
        Ok(u) if !u.is_empty() => Ok(Some(u)),
        _ => Err(Error::msg(
            "makepkg cannot run as root; convert as a normal user, or re-run via sudo so SUDO_USER is set",
        )),
    }
}

fn euid_is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| o.stdout == b"0\n")
        .unwrap_or(false)
}

fn chown_tree(path: &Path, user: &str) -> Result<()> {
    let status = Command::new("chown")
        .args(["-R", user, "--"])
        .arg(path)
        .status()
        .map_err(|e| Error::msg(format!("chown {user}: {e}")))?;
    if !status.success() {
        return Err(Error::msg(format!(
            "chown -R {user} {} failed",
            path.display()
        )));
    }
    Ok(())
}

fn is_sync_flag(arg: &str) -> bool {
    arg == "-s" || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('s'))
}

fn find_pkg(workdir: &Path) -> Result<PathBuf> {
    let mut found = Vec::new();
    for ent in std::fs::read_dir(workdir)? {
        let ent = ent?;
        let name = ent.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_pkg_archive(name) {
            found.push(ent.path());
        }
    }
    found.sort();
    found
        .into_iter()
        .next()
        .ok_or_else(|| Error::msg("makepkg produced no *.pkg.tar.*"))
}

fn is_pkg_archive(name: &str) -> bool {
    match name.split_once(".pkg.tar.") {
        Some((pre, post)) => !pre.is_empty() && !post.is_empty(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_root_runs_as_self() {
        // Default unit-test process is not root.
        if euid_is_root() {
            return;
        }
        assert!(matches!(makepkg_run_as(), Ok(None)));
    }

    #[test]
    #[ignore = "needs makepkg"]
    fn makepkg_trivial() {
        let wd = std::env::temp_dir().join(format!("packager-mk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wd);
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(
            wd.join("PKGBUILD"),
            "pkgname=pkgtst\npkgver=1\npkgrel=1\narch=('any')\npackage() { :; }\n",
        )
        .unwrap();
        let p = makepkg(&wd).unwrap();
        assert!(p.exists(), "{}", p.display());
        let _ = std::fs::remove_dir_all(wd);
    }
}
