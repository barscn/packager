//! Per-user install state JSON (sudo-aware data directory).

use crate::error::{Error, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

static TEST_DATA_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Record {
    pub pkgname: String,
    pub pkgver: String,
    pub arch: String,
    pub source_name: String,
    pub source_checksum: String,
    pub format: String,
    pub installed_at: String, // RFC3339
    pub allow_scripts: bool,
    #[serde(rename = "force")]
    pub forced: bool,
    pub workdir: String,
}

/// Override data dir for unit tests. Pass `None` to clear.
pub fn set_data_dir_for_test(dir: Option<PathBuf>) {
    *TEST_DATA_DIR.lock().unwrap() = dir;
}

pub fn data_dir() -> PathBuf {
    if let Some(dir) = TEST_DATA_DIR.lock().unwrap().clone() {
        return dir;
    }

    // Only when invoked via sudo: prefer SUDO_USER; empty SUDO_USER → logname.
    // Unset SUDO_USER is a normal (non-sudo) run — do not consult logname.
    if let Some(user) = sudo_invoking_user() {
        if let Some(home) = home_for_user(&user) {
            return home.join(".local/share/packager/installed");
        }
    }

    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("packager/installed");
        }
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    PathBuf::from(home).join(".local/share/packager/installed")
}

pub fn write(r: &Record) -> Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let path = record_path(&r.pkgname);
    let data = serde_json::to_vec_pretty(r).map_err(|e| Error::msg(e.to_string()))?;
    std::fs::write(path, data)?;
    Ok(())
}

pub fn read(pkgname: &str) -> Result<Record> {
    let path = record_path(pkgname);
    let data = std::fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("state record not found: {pkgname}"),
            ))
        } else {
            Error::Io(e)
        }
    })?;
    serde_json::from_slice(&data).map_err(|e| Error::msg(e.to_string()))
}

pub fn list() -> Result<Vec<Record>> {
    let dir = data_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let data = std::fs::read(&path)?;
        let rec: Record =
            serde_json::from_slice(&data).map_err(|e| Error::msg(e.to_string()))?;
        out.push(rec);
    }
    out.sort_by(|a, b| a.pkgname.cmp(&b.pkgname));
    Ok(out)
}

pub fn delete(pkgname: &str) -> Result<()> {
    let path = record_path(pkgname);
    std::fs::remove_file(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("state record not found: {pkgname}"),
            ))
        } else {
            Error::Io(e)
        }
    })?;
    Ok(())
}

/// SHA-256 hex digest of file contents at `path`.
pub fn source_sum(path: &Path) -> Result<String> {
    let data = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(format!("{:x}", hasher.finalize()))
}

fn record_path(pkgname: &str) -> PathBuf {
    data_dir().join(format!("{pkgname}.json"))
}

/// User whose home should hold state under sudo.
/// - `SUDO_USER` non-empty → that user
/// - `SUDO_USER` set but empty → `logname` (sudo-path fallback)
/// - `SUDO_USER` unset → `None` (normal run; use XDG/`HOME`)
fn sudo_invoking_user() -> Option<String> {
    match std::env::var("SUDO_USER") {
        Ok(u) if !u.is_empty() => Some(u),
        Ok(_) => logname_user(),
        Err(_) => None,
    }
}

fn logname_user() -> Option<String> {
    let out = Command::new("logname").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8(out.stdout).ok()?;
    let name = name.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Home directory from `getent passwd` field 6 (index 5).
fn home_for_user(user: &str) -> Option<PathBuf> {
    let out = Command::new("getent")
        .args(["passwd", user])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8(out.stdout).ok()?;
    let home = line.trim().split(':').nth(5)?;
    if home.is_empty() {
        None
    } else {
        Some(PathBuf::from(home))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_delete() {
        let dir = std::env::temp_dir().join(format!("packager-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        set_data_dir_for_test(Some(dir.clone()));
        let r = Record {
            pkgname: "hello".into(),
            pkgver: "1.0".into(),
            arch: "x86_64".into(),
            source_name: "hello.deb".into(),
            source_checksum: String::new(),
            format: "deb".into(),
            installed_at: String::new(),
            allow_scripts: false,
            forced: false,
            workdir: String::new(),
        };
        write(&r).unwrap();
        let got = read("hello").unwrap();
        assert_eq!(got.pkgname, "hello");
        assert_eq!(got.source_name, "hello.deb");
        assert_eq!(list().unwrap().len(), 1);
        delete("hello").unwrap();
        assert!(read("hello").is_err());
        set_data_dir_for_test(None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn data_dir_sudo_user() {
        set_data_dir_for_test(None);
        let getent = std::process::Command::new("getent")
            .args(["passwd", "nobody"])
            .output()
            .ok();
        let home = getent
            .and_then(|o| {
                String::from_utf8(o.stdout)
                    .ok()
                    .and_then(|l| l.split(':').nth(5).map(|s| s.to_string()))
            })
            .unwrap_or_else(|| "/usr/share/empty".into());
        let expected = PathBuf::from(&home).join(".local/share/packager/installed");
        std::env::set_var("SUDO_USER", "nobody");
        let d = data_dir();
        std::env::remove_var("SUDO_USER");
        let ds = d.to_string_lossy();
        assert!(!ds.contains("/root/"), "{ds}");
        assert_eq!(d, expected, "expected {expected:?}, got {d:?}");
    }

    #[test]
    fn data_dir_no_sudo_uses_xdg() {
        set_data_dir_for_test(None);
        let prev_sudo = std::env::var("SUDO_USER").ok();
        let prev_xdg = std::env::var("XDG_DATA_HOME").ok();
        std::env::remove_var("SUDO_USER");
        let xdg = std::env::temp_dir().join(format!("packager-xdg-{}", std::process::id()));
        std::env::set_var("XDG_DATA_HOME", &xdg);
        let d = data_dir();
        // restore env
        match prev_sudo {
            Some(v) => std::env::set_var("SUDO_USER", v),
            None => std::env::remove_var("SUDO_USER"),
        }
        match prev_xdg {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        set_data_dir_for_test(None);
        assert_eq!(d, xdg.join("packager/installed"));
    }
}
