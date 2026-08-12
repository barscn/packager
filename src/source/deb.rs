//! Debian `.deb` metadata via `bsdtar` (`control.tar` gz / xz / zst / plain).

use super::extract;
use crate::error::Result;
use crate::ident;
use crate::source::{Package, Script};
use std::path::Path;

const SCRIPT_NAMES: &[&str] = &["preinst", "postinst", "prerm", "postrm"];

/// Parse metadata from a `.deb` at `path`.
pub fn parse_meta(path: &Path) -> Result<Package> {
    let member = extract::deb_ar_member(path, "control.tar")?;
    let control_tar = extract::deb_ar_member_bytes(path, &member)?;

    let mut control_text = String::new();
    let mut scripts = Vec::new();
    for name in extract::tar_listing(&control_tar)? {
        if name == "control" {
            if let Some(bytes) = extract::tar_file(&control_tar, &name)? {
                control_text = String::from_utf8_lossy(&bytes).into_owned();
            }
        } else if SCRIPT_NAMES.contains(&name.as_str()) {
            if let Some(bytes) = extract::tar_file(&control_tar, &name)? {
                scripts.push(Script {
                    name,
                    body: String::from_utf8_lossy(&bytes).into_owned(),
                });
            }
        }
    }

    let fields = parse_control_fields(&control_text);
    let (epoch, raw_version) =
        split_version(fields.get("Version").map(String::as_str).unwrap_or(""));

    Ok(Package {
        format: ident::Format::Deb,
        path: path.to_path_buf(),
        raw_name: fields.get("Package").cloned().unwrap_or_default(),
        raw_version,
        epoch,
        raw_arch: fields.get("Architecture").cloned().unwrap_or_default(),
        depends: parse_dep_list(fields.get("Depends").map(String::as_str).unwrap_or("")),
        provides: parse_dep_list(fields.get("Provides").map(String::as_str).unwrap_or("")),
        conflicts: parse_dep_list(fields.get("Conflicts").map(String::as_str).unwrap_or("")),
        replaces: parse_dep_list(fields.get("Replaces").map(String::as_str).unwrap_or("")),
        scripts,
        file_list: Vec::new(),
    })
}

fn parse_control_fields(text: &str) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    let mut key = String::new();
    let mut val = String::new();
    for line in text.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if !key.is_empty() {
                val.push(' ');
                val.push_str(line.trim());
            }
            continue;
        }
        if !key.is_empty() {
            map.insert(std::mem::take(&mut key), std::mem::take(&mut val));
        }
        if let Some((k, v)) = line.split_once(':') {
            key = k.trim().to_string();
            val = v.trim().to_string();
        }
    }
    if !key.is_empty() {
        map.insert(key, val);
    }
    map
}

/// `1:2.0` → (`1`, `2.0`); no epoch → (`""`, full version).
fn split_version(raw: &str) -> (String, String) {
    match raw.split_once(':') {
        Some((epoch, ver)) => (epoch.to_string(), ver.to_string()),
        None => (String::new(), raw.to_string()),
    }
}

/// Split on `,`, strip `(…)` version constraints and `[arch]` qualifiers.
fn parse_dep_list(s: &str) -> Vec<String> {
    if s.trim().is_empty() {
        return Vec::new();
    }
    s.split(',')
        .filter_map(|part| {
            let mut t = part.trim().to_string();
            if t.is_empty() {
                return None;
            }
            // strip [arch]
            if let Some(i) = t.find('[') {
                if t.ends_with(']') {
                    t.truncate(i);
                    t = t.trim_end().to_string();
                }
            }
            // strip (constraint)
            if let Some(i) = t.find('(') {
                if t.contains(')') {
                    t.truncate(i);
                    t = t.trim_end().to_string();
                }
            }
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use tar::{Builder, EntryType, Header};

    fn control_text() -> &'static str {
        "Package: vendorapp\nVersion: 2.1\nArchitecture: amd64\nDepends: libc6\n"
    }

    fn control_tar(control: &str) -> Vec<u8> {
        let mut builder = Builder::new(Vec::new());
        let mut header = Header::new_gnu();
        header.set_path("./control").unwrap();
        header.set_entry_type(EntryType::Regular);
        header.set_size(control.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, control.as_bytes()).unwrap();
        builder.into_inner().unwrap()
    }

    fn write_ar(path: &std::path::Path, members: &[(&str, &[u8])]) {
        let mut out = std::fs::File::create(path).unwrap();
        out.write_all(b"!<arch>\n").unwrap();
        for (name, data) in members {
            let mut header = [b' '; 60];
            header[..name.len()].copy_from_slice(name.as_bytes());
            header[16..17].copy_from_slice(b"0");
            let size = data.len().to_string();
            header[48..48 + size.len()].copy_from_slice(size.as_bytes());
            header[58] = b'`';
            header[59] = b'\n';
            out.write_all(&header).unwrap();
            out.write_all(data).unwrap();
            if data.len() % 2 == 1 {
                out.write_all(b"\n").unwrap();
            }
        }
    }

    fn parse_member(member: &str, blob: &[u8]) -> Package {
        let dir = std::env::temp_dir().join(format!(
            "packager-ctrl-{}-{}",
            std::process::id(),
            member.replace('.', "_")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.deb");
        write_ar(
            &path,
            &[("debian-binary", b"2.0\n".as_slice()), (member, blob)],
        );
        let pkg = parse_meta(&path).unwrap();
        let _ = std::fs::remove_dir_all(dir);
        pkg
    }

    fn compress(tool: &str, args: &[&str], data: &[u8]) -> Option<Vec<u8>> {
        let mut child = Command::new(tool)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;
        child.stdin.take()?.write_all(data).ok()?;
        let out = child.wait_with_output().ok()?;
        out.status.success().then_some(out.stdout)
    }

    #[test]
    fn parse_control_tar_plain() {
        let pkg = parse_member("control.tar", &control_tar(control_text()));
        assert_eq!(pkg.raw_name, "vendorapp");
        assert_eq!(pkg.raw_version, "2.1");
        assert_eq!(pkg.depends, ["libc6"]);
    }

    #[test]
    fn parse_control_tar_xz_zst() {
        let tar = control_tar(control_text());
        if let Some(xz) = compress("xz", &["-c"], &tar) {
            let pkg = parse_member("control.tar.xz", &xz);
            assert_eq!(pkg.raw_name, "vendorapp");
            assert_eq!(pkg.raw_version, "2.1");
        }
        if let Some(zst) = compress("zstd", &["-c"], &tar) {
            let pkg = parse_member("control.tar.zst", &zst);
            assert_eq!(pkg.raw_name, "vendorapp");
            assert_eq!(pkg.raw_version, "2.1");
        }
    }
}
