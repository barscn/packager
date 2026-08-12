//! List and extract package payloads via `bsdtar`.

use super::{detect, Package};
use crate::error::{Error, Result};
use crate::ident;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// File names in the payload only (no write). Paths are relative, no `./` prefix.
pub fn list_payload(path: &Path) -> Result<Vec<String>> {
    match detect(path)? {
        ident::Format::Deb => list_deb(path),
        ident::Format::Rpm => list_from_bsdtar(path),
    }
}

/// Extract payload into `dest` and set `pkg.file_list` (same as [`list_payload`]).
pub fn extract(pkg: &mut Package, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    match pkg.format {
        ident::Format::Deb => extract_deb(&pkg.path, dest)?,
        ident::Format::Rpm => extract_rpm(&pkg.path, dest)?,
    }
    pkg.file_list = list_payload(&pkg.path)?;
    Ok(())
}

fn list_deb(path: &Path) -> Result<Vec<String>> {
    let member = data_member(path)?;
    let data = bsdtar_stdout(["-O", "-xf"], &[path.as_os_str(), member.as_ref()])?;
    let listing = bsdtar_stdin_stdout(&["-tf", "-"], &data)?;
    Ok(normalize_listing(&listing))
}

fn extract_deb(path: &Path, dest: &Path) -> Result<()> {
    let member = data_member(path)?;
    let data = bsdtar_stdout(["-O", "-xf"], &[path.as_os_str(), member.as_ref()])?;
    bsdtar_stdin_to_dest(&data, dest)
}

fn extract_rpm(path: &Path, dest: &Path) -> Result<()> {
    let out = Command::new("bsdtar")
        .args(["-xf"])
        .arg(path)
        .arg("-C")
        .arg(dest)
        .output()
        .map_err(|e| Error::msg(format!("bsdtar: {e}")))?;
    if !out.status.success() {
        return Err(Error::msg(format!(
            "bsdtar failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

fn list_from_bsdtar(path: &Path) -> Result<Vec<String>> {
    let out = Command::new("bsdtar")
        .arg("-tf")
        .arg(path)
        .output()
        .map_err(|e| Error::msg(format!("bsdtar: {e}")))?;
    if !out.status.success() {
        return Err(Error::msg(format!(
            "bsdtar failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(normalize_listing(&out.stdout))
}

fn data_member(path: &Path) -> Result<String> {
    deb_ar_member(path, "data.tar")
}

/// Locate an ar member named `stem` or `stem.*` (e.g. `control.tar`, `data.tar`).
pub(crate) fn deb_ar_member(path: &Path, stem: &str) -> Result<String> {
    let out = Command::new("bsdtar")
        .arg("-tf")
        .arg(path)
        .output()
        .map_err(|e| Error::msg(format!("bsdtar: {e}")))?;
    if !out.status.success() {
        return Err(Error::msg(format!(
            "bsdtar failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let prefix = format!("{stem}.");
    text.lines()
        .map(|l| l.trim().trim_end_matches('/'))
        .find(|n| *n == stem || n.starts_with(&prefix))
        .map(|s| s.to_string())
        .ok_or_else(|| Error::msg(format!("deb missing {stem}*")))
}

/// Bytes of one ar member (`bsdtar -O -xf`, any compression).
pub(crate) fn deb_ar_member_bytes(path: &Path, member: &str) -> Result<Vec<u8>> {
    bsdtar_stdout(["-O", "-xf"], &[path.as_os_str(), member.as_ref()])
}

/// Normalized paths inside a tar stream (gz / xz / zst / plain).
pub(crate) fn tar_listing(tar: &[u8]) -> Result<Vec<String>> {
    Ok(normalize_listing(&bsdtar_stdin_stdout(&["-tf", "-"], tar)?))
}

/// File contents from a tar stream. `name` is the listing path (`control`, not `./control`).
pub(crate) fn tar_file(tar: &[u8], name: &str) -> Result<Option<Vec<u8>>> {
    let listing = tar_listing(tar)?;
    if !listing.iter().any(|n| n == name) {
        return Ok(None);
    }
    for candidate in [name, &format!("./{name}")] {
        if let Ok(bytes) = bsdtar_stdin_stdout(&["-O", "-xf", "-", "--", candidate], tar) {
            return Ok(Some(bytes));
        }
    }
    Err(Error::msg(format!("bsdtar could not extract {name}")))
}

fn bsdtar_stdout(
    fixed: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
    rest: &[&std::ffi::OsStr],
) -> Result<Vec<u8>> {
    let out = Command::new("bsdtar")
        .args(fixed)
        .args(rest)
        .output()
        .map_err(|e| Error::msg(format!("bsdtar: {e}")))?;
    if !out.status.success() {
        return Err(Error::msg(format!(
            "bsdtar failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(out.stdout)
}

fn bsdtar_stdin_stdout(args: &[&str], stdin_data: &[u8]) -> Result<Vec<u8>> {
    let mut child = Command::new("bsdtar")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::msg(format!("bsdtar: {e}")))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::msg("bsdtar missing stdin"))?;
        stdin
            .write_all(stdin_data)
            .map_err(|e| Error::msg(format!("bsdtar: {e}")))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| Error::msg(format!("bsdtar: {e}")))?;
    if !out.status.success() {
        return Err(Error::msg(format!(
            "bsdtar failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(out.stdout)
}

fn bsdtar_stdin_to_dest(stdin_data: &[u8], dest: &Path) -> Result<()> {
    let mut child = Command::new("bsdtar")
        .args(["-xf", "-", "-C"])
        .arg(dest)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::msg(format!("bsdtar: {e}")))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::msg("bsdtar missing stdin"))?;
        stdin
            .write_all(stdin_data)
            .map_err(|e| Error::msg(format!("bsdtar: {e}")))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| Error::msg(format!("bsdtar: {e}")))?;
    if !out.status.success() {
        return Err(Error::msg(format!(
            "bsdtar failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

fn normalize_listing(stdout: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| {
            let s = line.trim();
            if s.is_empty() || s == "." {
                return None;
            }
            let s = s.trim_start_matches("./");
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        })
        .collect()
}
