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
        None => {
            let mut cmd = Command::new("makepkg");
            cmd.args(args)
                .current_dir(workdir)
                .env("PACKAGER", crate::ident::PACKAGER_FIELD)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            cmd.status()
                .map_err(|e| Error::msg(format!("makepkg: {e}")))?
        }
        Some(user) => {
            // Only chown files this run created — never `chown -R` a user `--workdir`.
            chown_created(workdir, &user)?;
            let mut cmd = Command::new("runuser");
            cmd.arg("-u")
                .arg(&user)
                .arg("--")
                .arg("makepkg")
                .args(args)
                .current_dir(workdir)
                .env("PACKAGER", crate::ident::PACKAGER_FIELD)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            cmd.status()
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

/// Chown only packager outputs (payload, PKGBUILD, scripts, `.install`).
/// Never `chown -R` the workdir itself — `--workdir` may contain other files.
/// The workdir inode is chowned non-recursively so `makepkg` can write the archive.
fn chown_created(workdir: &Path, user: &str) -> Result<()> {
    chown_path(workdir, user, false)?;
    for name in ["payload", "PKGBUILD", "scripts.orig"] {
        let p = workdir.join(name);
        if p.exists() {
            chown_path(&p, user, p.is_dir())?;
        }
    }
    if let Ok(rd) = std::fs::read_dir(workdir) {
        for ent in rd.flatten() {
            let name = ent.file_name();
            if name.to_str().is_some_and(|n| n.ends_with(".install")) {
                chown_path(&ent.path(), user, false)?;
            }
        }
    }
    Ok(())
}

fn chown_path(path: &Path, user: &str, recursive: bool) -> Result<()> {
    let mut cmd = Command::new("chown");
    if recursive {
        cmd.arg("-R");
    }
    let status = cmd
        .arg(user)
        .arg("--")
        .arg(path)
        .status()
        .map_err(|e| Error::msg(format!("chown {user}: {e}")))?;
    if !status.success() {
        return Err(Error::msg(format!(
            "chown {user} {} failed",
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
        if !is_pkg_archive(name) {
            continue;
        }
        let mtime = ent.metadata().and_then(|m| m.modified()).ok();
        found.push((ent.path(), name.to_string(), mtime));
    }
    if found.is_empty() {
        return Err(Error::msg("makepkg produced no *.pkg.tar.*"));
    }

    let prefix = pkgbuild_pkg_prefix(workdir);
    if let Some(prefix) = prefix {
        let mut hits: Vec<_> = found
            .iter()
            .filter(|(_, name, _)| name.starts_with(&prefix))
            .cloned()
            .collect();
        if !hits.is_empty() {
            sort_pkgs_newest_first(&mut hits);
            return Ok(hits[0].0.clone());
        }
    }

    sort_pkgs_newest_first(&mut found);
    Ok(found[0].0.clone())
}

fn sort_pkgs_newest_first(found: &mut [(PathBuf, String, Option<std::time::SystemTime>)]) {
    found.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
}

/// `{pkgname}-{pkgver}` from the generated PKGBUILD, if present.
fn pkgbuild_pkg_prefix(workdir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(workdir.join("PKGBUILD")).ok()?;
    let mut name = None;
    let mut ver = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("pkgname=") {
            if name.is_none() {
                name = Some(unquote_assign(v));
            }
        } else if let Some(v) = line.strip_prefix("pkgver=") {
            if ver.is_none() {
                ver = Some(unquote_assign(v));
            }
        }
    }
    Some(format!("{}-{}", name?, ver?))
}

fn unquote_assign(v: &str) -> String {
    v.trim().trim_matches(|c| c == '\'' || c == '"').to_string()
}

fn is_pkg_archive(name: &str) -> bool {
    if name.ends_with(".sig") {
        return false;
    }
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

    fn touch(path: &Path, stamp: &str) {
        std::fs::write(path, b"").unwrap();
        let st = Command::new("touch")
            .args(["-d", stamp, "--"])
            .arg(path)
            .status();
        assert!(
            st.map(|s| s.success()).unwrap_or(false),
            "touch {}",
            path.display()
        );
    }

    #[test]
    fn find_pkg_prefers_name_prefix_ignores_sig() {
        let wd = std::env::temp_dir().join(format!("packager-fp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wd);
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("PKGBUILD"), "pkgname=hello\npkgver=1.0\npkgrel=1\n").unwrap();
        // Lexicographically first, and older, so a naive sort would pick this.
        touch(
            &wd.join("aaa-9.0-1-x86_64.pkg.tar.zst"),
            "2000-01-01T00:00:00Z",
        );
        touch(
            &wd.join("hello-1.0-1-x86_64.pkg.tar.zst"),
            "2001-01-01T00:00:00Z",
        );
        touch(
            &wd.join("hello-1.0-1-x86_64.pkg.tar.zst.sig"),
            "2002-01-01T00:00:00Z",
        );
        let p = find_pkg(&wd).unwrap();
        assert_eq!(
            p.file_name().unwrap().to_str().unwrap(),
            "hello-1.0-1-x86_64.pkg.tar.zst"
        );
        let _ = std::fs::remove_dir_all(wd);
    }

    #[test]
    fn find_pkg_falls_back_to_newest() {
        let wd = std::env::temp_dir().join(format!("packager-fpn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wd);
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("PKGBUILD"), "pkgname=hello\npkgver=1.0\n").unwrap();
        touch(
            &wd.join("other-2.0-1-x86_64.pkg.tar.zst"),
            "2010-01-01T00:00:00Z",
        );
        touch(&wd.join("zzz-3.0-1-any.pkg.tar.xz"), "2020-01-01T00:00:00Z");
        touch(
            &wd.join("zzz-3.0-1-any.pkg.tar.xz.sig"),
            "2021-01-01T00:00:00Z",
        );
        let p = find_pkg(&wd).unwrap();
        assert_eq!(
            p.file_name().unwrap().to_str().unwrap(),
            "zzz-3.0-1-any.pkg.tar.xz"
        );
        let _ = std::fs::remove_dir_all(wd);
    }

    #[test]
    fn is_pkg_archive_skips_sig() {
        assert!(is_pkg_archive("hello-1.0-1-x86_64.pkg.tar.zst"));
        assert!(!is_pkg_archive("hello-1.0-1-x86_64.pkg.tar.zst.sig"));
        assert!(!is_pkg_archive("notes.txt"));
    }
}
