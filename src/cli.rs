//! CLI flags, subcommands, and the scaffold pipeline.

use crate::depmap;
use crate::error::{Error, Result};
use crate::hooks;
use crate::ident;
use crate::lookup;
use crate::pkgbuild;
use crate::preview;
use crate::source;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub cmd: String, // install|convert|scaffold|status|forget
    pub file: Option<PathBuf>,
    pub pkg: Option<String>,
    pub force: bool,
    pub allow_scripts: bool,
    pub yes: bool,
    pub workdir: Option<PathBuf>,
}

/// Optional handler invoked after a successful parse (tests / later pipeline).
pub static HANDLE: Mutex<Option<fn(&Config) -> i32>> = Mutex::new(None);

pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Config> {
    let mut force = false;
    let mut allow_scripts = false;
    let mut yes = false;
    let mut workdir: Option<PathBuf> = None;
    let mut positionals: Vec<String> = Vec::new();

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--force" => force = true,
            "--allow-scripts" => allow_scripts = true,
            "--yes" | "-y" => yes = true,
            "--workdir" => {
                let dir = iter
                    .next()
                    .ok_or_else(|| Error::msg("usage: --workdir requires DIR"))?;
                workdir = Some(PathBuf::from(dir));
            }
            s if s.starts_with('-') => {
                return Err(Error::msg(format!("unknown flag: {s}")));
            }
            _ => positionals.push(arg),
        }
    }

    let mut pos = positionals.into_iter();
    let first = pos
        .next()
        .ok_or_else(|| Error::msg("usage: packager [install|convert|scaffold|status|forget] …"))?;

    let (cmd, file, pkg) = match first.as_str() {
        "install" | "convert" | "scaffold" => {
            let f = pos
                .next()
                .ok_or_else(|| Error::msg(format!("usage: packager {first} <file.deb|.rpm>")))?;
            (first, Some(PathBuf::from(f)), None)
        }
        "status" => (first, None, None),
        "forget" => {
            let name = pos
                .next()
                .ok_or_else(|| Error::msg("usage: packager forget <pkg>"))?;
            (first, None, Some(name))
        }
        s if s.ends_with(".deb") || s.ends_with(".rpm") => {
            ("install".into(), Some(PathBuf::from(first)), None)
        }
        _ => {
            return Err(Error::msg(
                "usage: packager [install|convert|scaffold|status|forget] …",
            ));
        }
    };

    Ok(Config {
        cmd,
        file,
        pkg,
        force,
        allow_scripts,
        yes,
        workdir,
    })
}

pub fn run<I: IntoIterator<Item = String>>(args: I) -> i32 {
    match parse_args(args) {
        Err(e) => {
            eprintln!("{e}");
            2
        }
        Ok(cfg) => {
            if let Some(f) = *HANDLE.lock().unwrap() {
                return f(&cfg);
            }
            match cfg.cmd.as_str() {
                "scaffold" => scaffold(&cfg),
                "install" | "convert" | "status" | "forget" => {
                    eprintln!("not implemented");
                    2
                }
                _ => {
                    eprintln!("not implemented");
                    2
                }
            }
        }
    }
}

fn fail(e: impl std::fmt::Display) -> i32 {
    eprintln!("{e}");
    1
}

fn default_workdir(pkgname: &str, pkgver: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    PathBuf::from(home)
        .join(".cache/packager")
        .join(format!("{pkgname}-{pkgver}"))
}

/// Directory members of a payload listing (trailing `/` or a prefix of another path).
fn listed_dir(path: &str, all: &[String]) -> bool {
    let p = path.trim_end_matches('/');
    if path.ends_with('/') || p.is_empty() || p == "." {
        return true;
    }
    let prefix = format!("{p}/");
    all.iter()
        .any(|other| other.trim_end_matches('/').starts_with(&prefix))
}

fn scaffold(cfg: &Config) -> i32 {
    let path = match &cfg.file {
        Some(p) => p,
        None => return fail("usage: packager scaffold <file.deb|.rpm>"),
    };

    let mut pkg = match source::parse_meta(path) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };

    match source::list_payload(path) {
        Ok(list) => pkg.file_list = list,
        Err(e) => return fail(e),
    }

    let pkgname = ident::normalize_name(&pkg.raw_name);
    let names = lookup::candidates(&pkgname, &pkg.raw_name);
    let (native, lookup_err) = match hooks::lookup_client().find(&names) {
        Ok(hits) => (hits, None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };

    let depends = hooks::with_resolver(|r| depmap::map_names(&pkg.depends, r));

    let file_list = pkg.file_list.clone();
    let report = preview::evaluate(preview::Input {
        pkg: &pkg,
        force: cfg.force,
        allow_scripts: cfg.allow_scripts,
        host_arch: hooks::host_arch(),
        native,
        lookup_err,
        depends: depends.clone(),
        elf: Vec::new(),
        file_owned_by: Box::new(move |rel| {
            // Skip listed directories so /usr and /usr/bin are not file conflicts.
            if listed_dir(rel, &file_list) {
                return None;
            }
            crate::pm::owned_by(Path::new(rel)).ok().flatten()
        }),
    });

    print!("{}", preview::format_report(&report));

    if report.verdict == preview::Verdict::Blocked {
        return 1;
    }

    if !cfg.yes && !hooks::confirm("Proceed? [Enter] ") {
        return 1;
    }

    let workdir = match &cfg.workdir {
        Some(w) => w.clone(),
        None => default_workdir(&report.pkgname, &report.pkgver),
    };
    if let Err(e) = source::extract(&mut pkg, &workdir.join("payload")) {
        return fail(e);
    }
    if let Err(e) = pkgbuild::write(
        &pkg,
        &depends,
        &pkgbuild::Options {
            allow_scripts: cfg.allow_scripts,
            workdir,
            payload_rel: "payload".into(),
        },
    ) {
        return fail(e);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::depmap;
    use crate::lookup;
    use std::path::{Path, PathBuf};

    #[test]
    fn parse_default_install() {
        let c = parse_args(["--force".into(), "foo.deb".into()]).unwrap();
        assert_eq!(c.cmd, "install");
        assert_eq!(c.file.as_deref(), Some(Path::new("foo.deb")));
        assert!(c.force);
    }

    #[test]
    fn parse_subcommands() {
        let c = parse_args([
            "convert".into(),
            "a.rpm".into(),
            "--allow-scripts".into(),
            "-y".into(),
            "--workdir".into(),
            "/tmp/w".into(),
        ])
        .unwrap();
        assert_eq!(c.cmd, "convert");
        assert_eq!(c.file.as_deref(), Some(Path::new("a.rpm")));
        assert!(c.allow_scripts && c.yes);
        assert_eq!(c.workdir.as_deref(), Some(Path::new("/tmp/w")));
        let c = parse_args(["forget".into(), "hello".into()]).unwrap();
        assert_eq!(c.cmd, "forget");
        assert_eq!(c.pkg.as_deref(), Some("hello"));
        let c = parse_args(["status".into()]).unwrap();
        assert_eq!(c.cmd, "status");
    }

    #[test]
    fn parse_missing_file() {
        assert!(parse_args(["scaffold".into()]).is_err());
    }

    fn write_hello_deb() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("packager-hello-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("hello_1.0_amd64.deb");
        crate::testpkg::write_deb(
            &p,
            &crate::testpkg::DebSpec {
                name: "hello".into(),
                version: "1.0".into(),
                arch: "amd64".into(),
                depends: "libc6".into(),
                files: vec![("./usr/bin/hello".into(), b"hi\n".to_vec())],
                postinst: Some("#!/bin/sh\nupdate-desktop-database\n".into()),
            },
        )
        .unwrap();
        p
    }

    fn none_lookup() -> lookup::Client {
        fn n(_: &str) -> crate::error::Result<Option<lookup::Hit>> {
            Ok(None)
        }
        lookup::Client { extra: n, aur: n }
    }

    struct NoneRes;
    impl depmap::Resolver for NoneRes {
        fn which(&self, _: &str) -> Option<(String, String)> {
            None
        }
    }

    #[test]
    fn scaffold_writes_pkgbuild_without_archive() {
        let _l = crate::hooks::set_lookup(none_lookup());
        let _r = crate::hooks::set_resolver(Box::new(NoneRes));
        let _c = crate::hooks::set_confirm(|_| panic!("confirm must not run with -y"));
        let wd = std::env::temp_dir().join(format!("packager-scaf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wd);
        let code = run([
            "scaffold".into(),
            "-y".into(),
            write_hello_deb().to_string_lossy().into(),
            "--workdir".into(),
            wd.to_string_lossy().into(),
        ]);
        assert_eq!(code, 0);
        assert!(wd.join("PKGBUILD").exists());
        assert!(wd.join("scripts.orig/postinst").exists());
        assert!(wd.join("payload/usr/bin/hello").exists());
        let pkgs: Vec<_> = std::fs::read_dir(&wd)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".pkg.tar"))
            .collect();
        assert!(pkgs.is_empty(), "{pkgs:?}");
        let _ = std::fs::remove_dir_all(wd);
    }

    #[test]
    fn scaffold_native_stop_writes_nothing() {
        fn hit(_: &str) -> crate::error::Result<Option<lookup::Hit>> {
            Ok(Some(lookup::Hit {
                name: "hello".into(),
                version: "2".into(),
                repo: "extra".into(),
            }))
        }
        fn n(_: &str) -> crate::error::Result<Option<lookup::Hit>> {
            Ok(None)
        }
        let _l = crate::hooks::set_lookup(lookup::Client { extra: hit, aur: n });
        let _r = crate::hooks::set_resolver(Box::new(NoneRes));
        let wd = std::env::temp_dir().join(format!("packager-scafn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wd);
        let code = run([
            "scaffold".into(),
            "-y".into(),
            write_hello_deb().to_string_lossy().into(),
            "--workdir".into(),
            wd.to_string_lossy().into(),
        ]);
        assert_eq!(code, 1);
        assert!(!wd.join("PKGBUILD").exists());
        assert!(!wd.join("payload").exists());
        let _ = std::fs::remove_dir_all(wd);
    }

    #[test]
    fn scaffold_lookup_fail_mentions_lookup() {
        fn boom(_: &str) -> crate::error::Result<Option<lookup::Hit>> {
            Err(crate::error::Error::msg("offline"))
        }
        fn n(_: &str) -> crate::error::Result<Option<lookup::Hit>> {
            Ok(None)
        }
        let _l = crate::hooks::set_lookup(lookup::Client {
            extra: boom,
            aur: n,
        });
        let _r = crate::hooks::set_resolver(Box::new(NoneRes));
        let wd = std::env::temp_dir().join(format!("packager-scafl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wd);
        let code = run([
            "scaffold".into(),
            "-y".into(),
            write_hello_deb().to_string_lossy().into(),
            "--workdir".into(),
            wd.to_string_lossy().into(),
        ]);
        assert_eq!(code, 1);
        assert!(!wd.join("PKGBUILD").exists());
        assert!(!wd.join("payload").exists());
        let _ = std::fs::remove_dir_all(wd);
    }

    #[test]
    fn scaffold_uses_candidates_alias() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SAW_CHROME: AtomicBool = AtomicBool::new(false);
        fn extra(name: &str) -> crate::error::Result<Option<lookup::Hit>> {
            if name == "google-chrome" {
                SAW_CHROME.store(true, Ordering::SeqCst);
            }
            Ok(None)
        }
        fn n(_: &str) -> crate::error::Result<Option<lookup::Hit>> {
            Ok(None)
        }
        let _l = crate::hooks::set_lookup(lookup::Client { extra, aur: n });
        let _r = crate::hooks::set_resolver(Box::new(NoneRes));
        let dir = std::env::temp_dir().join(format!("packager-alias-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let deb = dir.join("google-chrome-stable_1_amd64.deb");
        crate::testpkg::write_deb(
            &deb,
            &crate::testpkg::DebSpec {
                name: "google-chrome-stable".into(),
                version: "1".into(),
                arch: "amd64".into(),
                depends: String::new(),
                files: vec![("./usr/bin/x".into(), b"x".to_vec())],
                postinst: None,
            },
        )
        .unwrap();
        let wd = dir.join("wd");
        let code = run([
            "scaffold".into(),
            "-y".into(),
            deb.to_string_lossy().into(),
            "--workdir".into(),
            wd.to_string_lossy().into(),
        ]);
        assert_eq!(code, 0);
        assert!(
            SAW_CHROME.load(Ordering::SeqCst),
            "candidates() must query alias google-chrome"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
