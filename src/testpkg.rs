//! Tiny package fixtures for tests (`.deb` now; `.rpm` later).

use crate::error::{Error, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use tar::{Builder, EntryType, Header};

pub struct DebSpec {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub depends: String,
    pub files: Vec<(String, Vec<u8>)>,
    pub postinst: Option<String>,
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
        let mode = if data.starts_with(b"#!") { 0o755 } else { 0o644 };
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
        let out = Command::new("ar").args(["t", path.to_str().unwrap()]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let listing = String::from_utf8_lossy(&out.stdout);
        for m in ["debian-binary", "control.tar.gz", "data.tar.gz"] {
            assert!(listing.contains(m), "{listing}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
