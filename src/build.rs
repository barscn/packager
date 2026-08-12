//! Run `makepkg` in a workdir (`-f` only, never `-s`).

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `makepkg -f` in `workdir`; returns the built `*.pkg.tar.*`.
pub fn makepkg(workdir: &Path) -> Result<PathBuf> {
    let args = ["-f"];
    if args.iter().any(|a| is_sync_flag(a)) {
        return Err(Error::msg("makepkg must not pass -s"));
    }
    let status = Command::new("makepkg")
        .args(args)
        .current_dir(workdir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| Error::msg(format!("makepkg: {e}")))?;
    if !status.success() {
        return Err(Error::msg("makepkg failed"));
    }
    find_pkg(workdir)
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
