//! Tiny package fixtures for tests (`.deb` / `.rpm`).

use crate::error::{Error, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tar::{Builder, EntryType, Header};

pub struct DebSpec {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub depends: String,
    pub files: Vec<(String, Vec<u8>)>,
    pub postinst: Option<String>,
}

pub struct RpmSpec {
    pub name: String,
    pub version: String,
    pub release: String,
    pub arch: String,
    pub requires: String,
    pub files: Vec<(String, Vec<u8>)>,
    pub post: Option<String>,
}

/// Write a minimal `.rpm` via `rpmbuild -bb` (temp `_topdir` tree).
pub fn write_rpm(path: &Path, spec: &RpmSpec) -> Result<()> {
    let top = std::env::temp_dir().join(format!(
        "packager-rpmbuild-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    for sub in ["BUILD", "RPMS", "SOURCES", "SPECS", "BUILDROOT", "SRPMS"] {
        std::fs::create_dir_all(top.join(sub))?;
    }

    // Stage payload files under SOURCES with stable basenames; map back in %install.
    let mut install_lines = Vec::new();
    let mut files_lines = Vec::new();
    for (i, (dest, data)) in spec.files.iter().enumerate() {
        let dest = dest.trim_start_matches("./");
        let dest = if dest.starts_with('/') {
            dest.to_string()
        } else {
            format!("/{dest}")
        };
        let src_name = format!("payload{i}");
        std::fs::write(top.join("SOURCES").join(&src_name), data)?;
        let mode = if data.starts_with(b"#!") {
            "755"
        } else {
            "644"
        };
        let parent = Path::new(&dest)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".into());
        install_lines.push(format!("mkdir -p \"%{{buildroot}}{parent}\""));
        install_lines.push(format!(
            "install -m {mode} \"%{{_sourcedir}}/{src_name}\" \"%{{buildroot}}{dest}\""
        ));
        files_lines.push(dest);
    }

    let requires_line = if spec.requires.trim().is_empty() {
        String::new()
    } else {
        format!("Requires: {}\n", spec.requires.trim())
    };

    let post_section = match &spec.post {
        Some(body) if !body.is_empty() => format!("\n%post\n{}\n", body.trim_end()),
        _ => String::new(),
    };

    let spec_body = format!(
        "Name: {name}\n\
         Version: {version}\n\
         Release: {release}\n\
         Summary: test fixture\n\
         License: MIT\n\
         BuildArch: {arch}\n\
         {requires}\
         \n\
         %description\n\
         test fixture\n\
         \n\
         %install\n\
         rm -rf \"%{{buildroot}}\"\n\
         {install}\n\
         \n\
         %files\n\
         {files}\n\
         {post}\n",
        name = spec.name,
        version = spec.version,
        release = spec.release,
        arch = spec.arch,
        requires = requires_line,
        install = install_lines.join("\n"),
        files = files_lines.join("\n"),
        post = post_section,
    );

    let spec_path = top.join("SPECS").join(format!("{}.spec", spec.name));
    std::fs::write(&spec_path, spec_body)?;

    let top_str = top.to_string_lossy();
    let out = Command::new("rpmbuild")
        .args([
            "-bb",
            "--define",
            &format!("_topdir {top_str}"),
            "--define",
            "debug_package %{nil}",
            "--define",
            "_build_id_links none",
        ])
        .arg(&spec_path)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::msg("rpmbuild not found; install rpm-tools")
            } else {
                Error::msg(format!("rpmbuild: {e}"))
            }
        })?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&top);
        return Err(Error::msg(format!(
            "rpmbuild failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }

    let produced =
        find_rpm(&top.join("RPMS"))?.ok_or_else(|| Error::msg("rpmbuild produced no .rpm"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&produced, path)?;
    let _ = std::fs::remove_dir_all(&top);
    Ok(())
}

fn find_rpm(dir: &Path) -> Result<Option<PathBuf>> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(cur) = stack.pop() {
        for entry in std::fs::read_dir(&cur)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("rpm") {
                return Ok(Some(p));
            }
        }
    }
    Ok(None)
}

/// Write a minimal `.deb` (GNU `ar` of `debian-binary`, `control.tar.gz`, `data.tar.gz`).
pub fn write_deb(path: &Path, spec: &DebSpec) -> Result<()> {
    let control = format!(
        "Package: {}\n\
         Version: {}\n\
         Architecture: {}\n\
         Maintainer: test <test@localhost>\n\
         Description: test fixture\n\
         Depends: {}\n",
        spec.name, spec.version, spec.arch, spec.depends
    );

    let control_tar_gz = build_control_tar_gz(&control, spec.postinst.as_deref())?;
    let data_tar_gz = build_data_tar_gz(&spec.files)?;

    let mut out = std::fs::File::create(path)?;
    write_ar(
        &mut out,
        &[
            ("debian-binary", b"2.0\n".as_slice()),
            ("control.tar.gz", control_tar_gz.as_slice()),
            ("data.tar.gz", data_tar_gz.as_slice()),
        ],
    )?;
    Ok(())
}

fn build_control_tar_gz(control: &str, postinst: Option<&str>) -> Result<Vec<u8>> {
    let enc = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = Builder::new(enc);
    append_regular(&mut builder, "./control", control.as_bytes(), 0o644)?;
    if let Some(script) = postinst {
        append_regular(&mut builder, "./postinst", script.as_bytes(), 0o755)?;
    }
    finish_gz_tar(builder)
}

fn build_data_tar_gz(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let enc = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = Builder::new(enc);
    let mut created_dirs = BTreeSet::new();
    for (path, data) in files {
        for dir in parent_dirs(path) {
            if created_dirs.insert(dir.clone()) {
                append_dir(&mut builder, &dir)?;
            }
        }
        let mode = if data.starts_with(b"#!") {
            0o755
        } else {
            0o644
        };
        append_regular(&mut builder, path, data, mode)?;
    }
    finish_gz_tar(builder)
}

fn finish_gz_tar(builder: Builder<GzEncoder<Vec<u8>>>) -> Result<Vec<u8>> {
    let enc = builder.into_inner().map_err(Error::from)?;
    Ok(enc.finish()?)
}

fn parent_dirs(path: &str) -> Vec<String> {
    let path = path.trim_start_matches("./").trim_end_matches('/');
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 1 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for part in &parts[..parts.len() - 1] {
        if cur.is_empty() {
            cur = format!("./{part}");
        } else {
            cur = format!("{cur}/{part}");
        }
        out.push(cur.clone());
    }
    out
}

fn append_regular<W: Write>(
    builder: &mut Builder<W>,
    path: &str,
    data: &[u8],
    mode: u32,
) -> Result<()> {
    let mut header = Header::new_gnu();
    header
        .set_path(path)
        .map_err(|e| Error::msg(e.to_string()))?;
    header.set_entry_type(EntryType::Regular);
    header.set_size(data.len() as u64);
    header.set_mode(mode);
    header.set_cksum();
    builder.append(&header, data)?;
    Ok(())
}

fn append_dir<W: Write>(builder: &mut Builder<W>, path: &str) -> Result<()> {
    let mut header = Header::new_gnu();
    header
        .set_path(path)
        .map_err(|e| Error::msg(e.to_string()))?;
    header.set_entry_type(EntryType::Directory);
    header.set_size(0);
    header.set_mode(0o755);
    header.set_cksum();
    builder.append(&header, std::io::empty())?;
    Ok(())
}

/// GNU `ar`: magic `!<arch>\n`, 60-byte headers, even-size padding with `\n`.
fn write_ar(w: &mut impl Write, members: &[(&str, &[u8])]) -> Result<()> {
    w.write_all(b"!<arch>\n")?;
    for (name, data) in members {
        write_ar_member(w, name, data)?;
    }
    Ok(())
}

fn write_ar_member(w: &mut impl Write, name: &str, data: &[u8]) -> Result<()> {
    let name_b = name.as_bytes();
    if name_b.len() > 16 {
        return Err(Error::msg(format!("ar member name too long: {name}")));
    }

    let mut header = [b' '; 60];
    header[..name_b.len()].copy_from_slice(name_b);

    // mtime (12), uid (6), gid (6), mode (8), size (10) — left-aligned decimals/octal
    write_ar_field(&mut header[16..28], b"0");
    write_ar_field(&mut header[28..34], b"0");
    write_ar_field(&mut header[34..40], b"0");
    write_ar_field(&mut header[40..48], b"100644");
    let size = data.len().to_string();
    write_ar_field(&mut header[48..58], size.as_bytes());
    header[58] = b'`';
    header[59] = b'\n';

    w.write_all(&header)?;
    w.write_all(data)?;
    if data.len() % 2 == 1 {
        w.write_all(b"\n")?;
    }
    Ok(())
}

fn write_ar_field(dst: &mut [u8], value: &[u8]) {
    let n = value.len().min(dst.len());
    dst[..n].copy_from_slice(&value[..n]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn write_deb_is_ar_archive() {
        let dir = std::env::temp_dir().join(format!("packager-testpkg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hello_1.0_amd64.deb");
        write_deb(
            &path,
            &DebSpec {
                name: "hello".into(),
                version: "1.0".into(),
                arch: "amd64".into(),
                depends: "libc6".into(),
                files: vec![("./usr/bin/hello".into(), b"#!/bin/sh\necho hi\n".to_vec())],
                postinst: Some("#!/bin/sh\nupdate-desktop-database\n".into()),
            },
        )
        .unwrap();
        let out = Command::new("ar")
            .args(["t", path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let listing = String::from_utf8_lossy(&out.stdout);
        for m in ["debian-binary", "control.tar.gz", "data.tar.gz"] {
            assert!(listing.contains(m), "{listing}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
