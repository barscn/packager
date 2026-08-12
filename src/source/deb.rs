//! Debian `.deb` metadata: GNU ar + control.tar.gz.

use crate::error::{Error, Result};
use crate::ident;
use crate::source::{Package, Script};
use flate2::read::GzDecoder;
use std::io::Read;
use std::path::Path;
use tar::Archive;

const AR_MAGIC: &[u8] = b"!<arch>\n";
const AR_HEADER_LEN: usize = 60;
const SCRIPT_NAMES: &[&str] = &["preinst", "postinst", "prerm", "postrm"];

/// Parse metadata from a `.deb` at `path`.
pub fn parse_meta(path: &Path) -> Result<Package> {
    let bytes = std::fs::read(path)?;
    let members = read_ar(&bytes)?;
    let control_gz = members
        .iter()
        .find(|(name, _)| name == "control.tar.gz" || name.starts_with("control.tar"))
        .map(|(_, data)| data.as_slice())
        .ok_or_else(|| Error::msg("deb missing control.tar.gz"))?;

    let mut control_text = String::new();
    let mut scripts = Vec::new();

    let decoder = GzDecoder::new(control_gz);
    let mut archive = Archive::new(decoder);
    for entry in archive.entries().map_err(|e| Error::msg(e.to_string()))? {
        let mut entry = entry.map_err(|e| Error::msg(e.to_string()))?;
        let path_name = entry
            .path()
            .map_err(|e| Error::msg(e.to_string()))?
            .to_string_lossy()
            .into_owned();
        let base = path_name.trim_start_matches("./");
        if base == "control" {
            entry
                .read_to_string(&mut control_text)
                .map_err(|e| Error::msg(e.to_string()))?;
        } else if SCRIPT_NAMES.contains(&base) {
            let mut body = String::new();
            entry
                .read_to_string(&mut body)
                .map_err(|e| Error::msg(e.to_string()))?;
            scripts.push(Script {
                name: base.to_string(),
                body,
            });
        }
    }

    let fields = parse_control_fields(&control_text);
    let (epoch, raw_version) = split_version(fields.get("Version").map(String::as_str).unwrap_or(""));

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

/// Minimal GNU ar reader: magic, 60-byte headers, even-size padding.
fn read_ar(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    if data.len() < AR_MAGIC.len() || &data[..AR_MAGIC.len()] != AR_MAGIC {
        return Err(Error::msg("not a GNU ar archive"));
    }
    let mut off = AR_MAGIC.len();
    let mut members = Vec::new();
    while off + AR_HEADER_LEN <= data.len() {
        let header = &data[off..off + AR_HEADER_LEN];
        off += AR_HEADER_LEN;

        let name = ar_field(&header[0..16]);
        let name = name.trim_end_matches('/').to_string();
        let size: usize = ar_field(&header[48..58])
            .parse()
            .map_err(|_| Error::msg(format!("bad ar size for {name}")))?;

        if off + size > data.len() {
            return Err(Error::msg(format!("ar member {name} truncated")));
        }
        let body = data[off..off + size].to_vec();
        off += size;
        if size % 2 == 1 {
            off += 1; // even-size padding
        }
        if !name.is_empty() {
            members.push((name, body));
        }
    }
    Ok(members)
}

fn ar_field(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
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
