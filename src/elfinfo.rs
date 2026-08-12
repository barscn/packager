//! ELF scan via `readelf` and foreign layout/ABI warnings.

use crate::error::{Error, Result};
use crate::ident;
use std::path::Path;
use std::process::Command;

/// Parsed ELF metadata for one file under an extracted tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub path: String,
    pub class: String, // "ELF32" | "ELF64"
    pub interpreter: String,
    pub needed: Vec<String>,
}

/// Walk regular files under `root`, skip non-ELF, parse with `readelf -h -d -l`.
pub fn scan(root: &Path) -> Result<Vec<Info>> {
    let mut out = Vec::new();
    walk_files(root, root, &mut out)?;
    Ok(out)
}

fn walk_files(root: &Path, dir: &Path, out: &mut Vec<Info>) -> Result<()> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk_files(root, &path, out)?;
        } else if ft.is_file() {
            if let Some(info) = scan_file(root, &path)? {
                out.push(info);
            }
        }
    }
    Ok(())
}

fn scan_file(root: &Path, path: &Path) -> Result<Option<Info>> {
    if !is_elf(path)? {
        return Ok(None);
    }
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let text = run_readelf(path)?;
    Ok(Some(parse_readelf(&rel, &text)))
}

fn is_elf(path: &Path) -> Result<bool> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut magic = [0u8; 4];
    match f.read_exact(&mut magic) {
        Ok(()) => Ok(&magic == b"\x7fELF"),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn run_readelf(path: &Path) -> Result<String> {
    let out = Command::new("readelf")
        .args(["-h", "-d", "-l"])
        .arg(path)
        .output()
        .map_err(|e| Error::msg(format!("readelf: {e}")))?;
    if !out.status.success() {
        return Err(Error::msg(format!(
            "readelf failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn parse_readelf(rel: &str, text: &str) -> Info {
    let mut class = String::new();
    let mut interpreter = String::new();
    let mut needed = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Class:") {
            class = rest.trim().to_string();
        } else if let Some(idx) = line.find("Requesting program interpreter:") {
            let rest = &line[idx + "Requesting program interpreter:".len()..];
            interpreter = rest
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
        } else if line.contains("(NEEDED)") {
            // Shared library: [libc.so.6]
            if let Some(start) = line.rfind('[') {
                if let Some(end) = line.rfind(']') {
                    if end > start {
                        needed.push(line[start + 1..end].to_string());
                    }
                }
            }
        }
    }

    Info {
        path: rel.to_string(),
        class,
        interpreter,
        needed,
    }
}

/// Layout / ABI warnings. Never rewrites paths.
pub fn layout_warnings(
    file_list: &[String],
    infos: &[Info],
    host: ident::Arch,
) -> Vec<String> {
    let mut warnings = Vec::new();

    for path in file_list {
        if path.contains("usr/lib/x86_64-linux-gnu") || path.contains("lib/x86_64-linux-gnu") {
            warnings.push(format!("Debian multiarch path: {path}"));
        }
        if path.contains("usr/lib64/") || path == "usr/lib64" || path.starts_with("usr/lib64/") {
            warnings.push(format!("Fedora lib64 path: {path}"));
        }
    }

    for info in infos {
        if info.class == "ELF32" && host == ident::Arch::X86_64 {
            warnings.push(format!("32-bit ELF on x86_64: {}", info.path));
        }
        if info.interpreter.is_empty() && !info.needed.is_empty() {
            warnings.push(format!("missing interpreter: {}", info.path));
        }
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::Arch;

    #[test]
    fn layout_warnings_detect() {
        let files = [
            "usr/lib/x86_64-linux-gnu/libvendor.so".into(),
            "usr/lib64/libfoo.so".into(),
            "usr/bin/hello".into(),
        ];
        let infos = [
            Info {
                path: "usr/bin/old".into(),
                class: "ELF32".into(),
                interpreter: "/lib/ld-linux.so.2".into(),
                needed: vec!["libc.so.6".into()],
            },
            Info {
                path: "usr/bin/broken".into(),
                class: "ELF64".into(),
                interpreter: String::new(),
                needed: vec!["libc.so.6".into()],
            },
        ];
        let ws = layout_warnings(&files, &infos, Arch::X86_64);
        for w in [
            "Debian multiarch path",
            "Fedora lib64 path",
            "32-bit ELF on x86_64",
            "missing interpreter",
        ] {
            assert!(ws.iter().any(|g| g.contains(w)), "missing {w} in {ws:?}");
        }
    }

    #[test]
    fn layout_does_not_rewrite() {
        let files = vec!["usr/lib/x86_64-linux-gnu/x.so".into()];
        let orig = files.clone();
        let _ = layout_warnings(&files, &[], Arch::X86_64);
        assert_eq!(files, orig);
    }

    #[test]
    fn scan_finds_elf() {
        if Command::new("readelf").arg("--version").output().is_err() {
            eprintln!("skip scan_finds_elf: readelf missing");
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "packager-elfinfo-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("usr/bin")).unwrap();
        // Copy a known host ELF into the tree
        let src = Path::new("/bin/ls");
        if !src.is_file() {
            eprintln!("skip scan_finds_elf: /bin/ls missing");
            return;
        }
        std::fs::copy(&src, dir.join("usr/bin/hello")).unwrap();
        std::fs::write(dir.join("usr/bin/not-elf"), b"hello").unwrap();

        let infos = scan(&dir).expect("scan");
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            infos.iter().any(|i| i.path == "usr/bin/hello" && i.class == "ELF64"),
            "infos={infos:?}"
        );
        assert!(
            !infos.iter().any(|i| i.path.contains("not-elf")),
            "non-ELF must be skipped: {infos:?}"
        );
        let hello = infos.iter().find(|i| i.path == "usr/bin/hello").unwrap();
        assert!(!hello.needed.is_empty(), "expected NEEDED: {hello:?}");
        assert!(!hello.interpreter.is_empty(), "expected interpreter: {hello:?}");
    }

}
