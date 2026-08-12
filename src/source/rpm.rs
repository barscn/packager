//! RPM metadata via `rpm -qp` (queryformat + `--scripts`).

use crate::error::{Error, Result};
use crate::ident;
use crate::source::{Package, Script};
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

/// Path/name of the `rpm` binary; tests may point this at a missing command.
/// Empty string means default `"rpm"`.
pub static RPM_BIN: Mutex<String> = Mutex::new(String::new());

fn rpm_bin() -> String {
    let g = RPM_BIN.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_empty() {
        "rpm".to_string()
    } else {
        g.clone()
    }
}

/// Parse metadata from a `.rpm` at `path`.
pub fn parse_meta(path: &Path) -> Result<Package> {
    let bin = rpm_bin();
    // rpm interprets `\n` escapes in --queryformat (not raw newlines).
    let qf = "%{NAME}\\n%{VERSION}\\n%{EPOCH}\\n%{ARCH}\\n--REQUIRES--\\n[%{REQUIRENAME}\\n]--PROVIDES--\\n[%{PROVIDENAME}\\n]--CONFLICTS--\\n[%{CONFLICTNAME}\\n]";

    let out = run_rpm(&bin, path, &["--queryformat", qf])?;
    let text = String::from_utf8_lossy(&out);
    let mut lines = text.lines();

    let raw_name = next_field(&mut lines, "NAME")?;
    let raw_version = next_field(&mut lines, "VERSION")?;
    let epoch_raw = next_field(&mut lines, "EPOCH")?;
    let raw_arch = next_field(&mut lines, "ARCH")?;

    let epoch = if epoch_raw.is_empty() || epoch_raw == "(none)" {
        String::new()
    } else {
        epoch_raw
    };

    let mut depends = Vec::new();
    let mut provides = Vec::new();
    let mut conflicts = Vec::new();
    let mut section = Section::None;

    for line in lines {
        match line {
            "--REQUIRES--" => section = Section::Requires,
            "--PROVIDES--" => section = Section::Provides,
            "--CONFLICTS--" => section = Section::Conflicts,
            _ => {
                let name = line.trim();
                if name.is_empty() {
                    continue;
                }
                match section {
                    Section::Requires => {
                        if !is_rpmlib(name) {
                            depends.push(name.to_string());
                        }
                    }
                    Section::Provides => provides.push(name.to_string()),
                    Section::Conflicts => conflicts.push(name.to_string()),
                    Section::None => {}
                }
            }
        }
    }

    let scripts = parse_scripts(&bin, path)?;

    Ok(Package {
        format: ident::Format::Rpm,
        path: path.to_path_buf(),
        raw_name,
        raw_version,
        epoch,
        raw_arch,
        depends,
        provides,
        conflicts,
        replaces: Vec::new(),
        scripts,
        file_list: Vec::new(),
    })
}

enum Section {
    None,
    Requires,
    Provides,
    Conflicts,
}

fn next_field<'a>(lines: &mut impl Iterator<Item = &'a str>, label: &str) -> Result<String> {
    lines
        .next()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| Error::msg(format!("rpm query missing {label}")))
}

fn is_rpmlib(name: &str) -> bool {
    name.starts_with("rpmlib(") && name.ends_with(')')
}

fn run_rpm(bin: &str, path: &Path, extra: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = Command::new(bin);
    cmd.arg("-qp");
    cmd.args(extra);
    cmd.arg(path);
    let out = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::msg("install rpm-tools")
        } else {
            Error::msg(format!("rpm: {e}"))
        }
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // Missing binary sometimes surfaces as failed status with empty/odd stderr.
        if stderr.contains("No such file") || stderr.contains("not found") {
            return Err(Error::msg(format!("install rpm-tools: {stderr}")));
        }
        return Err(Error::msg(format!(
            "rpm failed: {}",
            if stderr.is_empty() {
                String::from_utf8_lossy(&out.stdout)
            } else {
                stderr
            }
        )));
    }
    Ok(out.stdout)
}

/// `rpm -qp --scripts` → `pre` / `post` / `preun` / `postun`.
fn parse_scripts(bin: &str, path: &Path) -> Result<Vec<Script>> {
    let out = run_rpm(bin, path, &["--scripts"])?;
    let text = String::from_utf8_lossy(&out);
    Ok(parse_scripts_text(&text))
}

fn parse_scripts_text(text: &str) -> Vec<Script> {
    let mut scripts = Vec::new();
    let mut cur_name: Option<String> = None;
    let mut cur_body = String::new();

    for line in text.lines() {
        if let Some(name) = script_header(line) {
            if let Some(n) = cur_name.take() {
                scripts.push(Script {
                    name: n,
                    body: std::mem::take(&mut cur_body),
                });
            }
            cur_name = Some(name);
            cur_body.clear();
        } else if cur_name.is_some() {
            cur_body.push_str(line);
            cur_body.push('\n');
        }
    }
    if let Some(n) = cur_name {
        scripts.push(Script {
            name: n,
            body: cur_body,
        });
    }
    scripts
}

fn script_header(line: &str) -> Option<String> {
    // e.g. "postinstall scriptlet (using /bin/sh):"
    let lower = line.to_ascii_lowercase();
    let name = if lower.starts_with("preinstall scriptlet") {
        "pre"
    } else if lower.starts_with("postinstall scriptlet") {
        "post"
    } else if lower.starts_with("preuninstall scriptlet") {
        "preun"
    } else if lower.starts_with("postuninstall scriptlet") {
        "postun"
    } else {
        return None;
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_rpmlib_and_parses_scripts() {
        assert!(is_rpmlib("rpmlib(CompressedFileNames)"));
        assert!(!is_rpmlib("glibc"));

        let text = "\
postinstall scriptlet (using /bin/sh):
ldconfig

preuninstall scriptlet (using /bin/sh):
true
";
        let scripts = parse_scripts_text(text);
        assert_eq!(scripts.len(), 2);
        assert_eq!(scripts[0].name, "post");
        assert!(scripts[0].body.contains("ldconfig"));
        assert_eq!(scripts[1].name, "preun");
    }
}
