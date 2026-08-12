//! Detect package format and parse metadata from `.deb` / `.rpm`.

use crate::error::{Error, Result};
use crate::ident;
use std::path::{Path, PathBuf};

mod deb;
mod extract;

pub use extract::{extract, list_payload};

#[derive(Clone, Debug)]
pub struct Script {
    pub name: String,
    pub body: String,
}

#[derive(Clone, Debug, Default)]
pub struct Package {
    pub format: ident::Format,
    pub path: PathBuf,
    pub raw_name: String,
    pub raw_version: String,
    pub epoch: String,
    pub raw_arch: String,
    pub depends: Vec<String>,
    pub provides: Vec<String>,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
    pub scripts: Vec<Script>,
    pub file_list: Vec<String>,
}

/// Detect format from file suffix (case-insensitive).
pub fn detect(path: &Path) -> Result<ident::Format> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name.ends_with(".deb") {
        Ok(ident::Format::Deb)
    } else if name.ends_with(".rpm") {
        Ok(ident::Format::Rpm)
    } else {
        Err(Error::msg("not a .deb/.rpm"))
    }
}

/// Parse package metadata; dispatches on [`detect`].
pub fn parse_meta(path: &Path) -> Result<Package> {
    match detect(path)? {
        ident::Format::Deb => deb::parse_meta(path),
        ident::Format::Rpm => Err(Error::msg("rpm parse not implemented")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testpkg::{write_deb, DebSpec};

    #[test]
    fn detect_suffix() {
        assert!(matches!(detect(Path::new("foo.deb")).unwrap(), ident::Format::Deb));
        assert!(matches!(detect(Path::new("foo.rpm")).unwrap(), ident::Format::Rpm));
        assert!(detect(Path::new("foo.tar")).is_err());
    }

    #[test]
    fn parse_deb_meta() {
        let dir = std::env::temp_dir().join(format!("packager-debmeta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hello_1.0_amd64.deb");
        write_deb(&path, &DebSpec {
            name: "hello".into(), version: "1.0".into(), arch: "amd64".into(),
            depends: "libc6, libgtk-3-0".into(),
            files: vec![("./usr/bin/hello".into(), b"hi\n".to_vec())],
            postinst: Some("#!/bin/sh\nupdate-desktop-database\n".into()),
        }).unwrap();
        let pkg = parse_meta(&path).unwrap();
        assert!(matches!(pkg.format, ident::Format::Deb));
        assert_eq!(pkg.raw_name, "hello");
        assert_eq!(pkg.raw_version, "1.0");
        assert_eq!(pkg.raw_arch, "amd64");
        assert_eq!(pkg.depends, ["libc6", "libgtk-3-0"]);
        assert_eq!(pkg.scripts.len(), 1);
        assert_eq!(pkg.scripts[0].name, "postinst");
        assert!(pkg.scripts[0].body.contains("update-desktop-database"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn extract_deb() {
        let dir = std::env::temp_dir().join(format!("packager-extract-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let deb = dir.join("hello_1.0_amd64.deb");
        crate::testpkg::write_deb(&deb, &crate::testpkg::DebSpec {
            name: "hello".into(), version: "1.0".into(), arch: "amd64".into(),
            depends: String::new(),
            files: vec![("./usr/bin/hello".into(), b"hi\n".to_vec())],
            postinst: None,
        }).unwrap();
        let mut pkg = parse_meta(&deb).unwrap();
        let dest = dir.join("out");
        extract(&mut pkg, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("usr/bin/hello")).unwrap(), "hi\n");
        assert!(pkg.file_list.iter().any(|f| f == "usr/bin/hello"), "{:?}", pkg.file_list);
        let listed = list_payload(&deb).unwrap();
        assert!(listed.iter().any(|f| f == "usr/bin/hello"), "{listed:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
