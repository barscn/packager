//! CLI flags, subcommands, and the scaffold/convert pipeline.

use crate::build;
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
                "scaffold" => pipeline(&cfg, false),
                "convert" => pipeline(&cfg, true),
                "install" | "status" | "forget" => {
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

/// Preview, then extract + write PKGBUILD. Convert also scans NEEDED and runs makepkg.
fn pipeline(cfg: &Config, convert: bool) -> i32 {
    let usage = if convert {
        "usage: packager convert <file.deb|.rpm>"
    } else {
        "usage: packager scaffold <file.deb|.rpm>"
    };
    let path = match &cfg.file {
        Some(p) => p,
        None => return fail(usage),
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

    let mut depends = hooks::with_resolver(|r| depmap::map_names(&pkg.depends, r));

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
    let payload = workdir.join("payload");
    if let Err(e) = source::extract(&mut pkg, &payload) {
        return fail(e);
    }
    if convert {
        match union_needed(&pkg.depends, &payload) {
            Ok(d) => depends = d,
            Err(e) => return fail(e),
        }
    }
    if let Err(e) = pkgbuild::write(
        &pkg,
        &depends,
        &pkgbuild::Options {
            allow_scripts: cfg.allow_scripts,
            workdir: workdir.clone(),
            payload_rel: "payload".into(),
        },
    ) {
        return fail(e);
    }
    if convert {
        if let Err(e) = build::makepkg(&workdir) {
            return fail(e);
        }
    }
    0
}

/// `needed_names` ∪ declared depends → `map_names`, adding new Extra hits.
fn union_needed(declared: &[String], payload: &Path) -> Result<depmap::Buckets> {
    let infos = hooks::scan(payload)?;
    let sonames: Vec<String> = infos
        .iter()
        .flat_map(|i| i.needed.iter().cloned())
        .collect();
    Ok(hooks::with_resolver(|r| {
        let mut b = depmap::map_names(declared, r);
        for n in depmap::needed_names(&sonames, r) {
            if !b.extra.iter().any(|e| e == &n) {
                b.extra.push(n);
            }
        }
        b
    }))
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

    fn has_makepkg() -> bool {
        std::process::Command::new("makepkg")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    #[ignore = "needs makepkg"]
    fn convert_builds_archive_without_install_script() {
        assert!(has_makepkg());
        let _l = crate::hooks::set_lookup(none_lookup());
        let _r = crate::hooks::set_resolver(Box::new(NoneRes));
        let wd = std::env::temp_dir().join(format!("packager-cv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wd);
        let code = run([
            "convert".into(),
            "-y".into(),
            write_hello_deb().to_string_lossy().into(),
            "--workdir".into(),
            wd.to_string_lossy().into(),
        ]);
        assert_eq!(code, 0);
        let pkg = std::fs::read_dir(&wd)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(".pkg.tar")
            })
            .expect("archive");
        let listing = String::from_utf8(
            std::process::Command::new("bsdtar")
                .args(["-tf", pkg.to_str().unwrap()])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert!(listing.contains("usr/bin/hello"), "{listing}");
        assert!(
            !listing
                .split('\n')
                .any(|l| l.ends_with(".INSTALL") || l == ".INSTALL"),
            "{listing}"
        );
        let _ = std::fs::remove_dir_all(wd);
    }

    #[test]
    #[ignore = "needs makepkg"]
    fn convert_allow_scripts_embeds_install() {
        assert!(has_makepkg());
        let _l = crate::hooks::set_lookup(none_lookup());
        let _r = crate::hooks::set_resolver(Box::new(NoneRes));
        let wd = std::env::temp_dir().join(format!("packager-cvs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wd);
        let code = run([
            "convert".into(),
            "-y".into(),
            "--allow-scripts".into(),
            write_hello_deb().to_string_lossy().into(),
            "--workdir".into(),
            wd.to_string_lossy().into(),
        ]);
        assert_eq!(code, 0);
        let pkg = std::fs::read_dir(&wd)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(".pkg.tar")
            })
            .expect("archive");
        let listing = String::from_utf8(
            std::process::Command::new("bsdtar")
                .args(["-tf", pkg.to_str().unwrap()])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert!(listing.contains(".INSTALL"), "{listing}");
        let _ = std::fs::remove_dir_all(wd);
    }

    #[test]
    fn convert_needed_union() {
        struct Res;
        impl depmap::Resolver for Res {
            fn which(&self, name: &str) -> Option<(String, String)> {
                if name == "libssl.so.3" {
                    Some(("openssl".into(), "extra".into()))
                } else {
                    None
                }
            }
        }
        let _l = crate::hooks::set_lookup(none_lookup());
        let _r = crate::hooks::set_resolver(Box::new(Res));
        let _s = crate::hooks::set_scan(|_| {
            Ok(vec![crate::elfinfo::Info {
                path: "usr/bin/hello".into(),
                class: "ELF64".into(),
                interpreter: "/lib64/ld-linux-x86-64.so.2".into(),
                needed: vec!["libssl.so.3".into()],
            }])
        });
        let wd = std::env::temp_dir().join(format!("packager-need-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wd);
        let code = run([
            "scaffold".into(),
            "-y".into(),
            write_hello_deb().to_string_lossy().into(),
            "--workdir".into(),
            wd.to_string_lossy().into(),
        ]);
        // convert path is what must scan; if makepkg missing, still run convert and
        // accept exit != 0 from makepkg *after* PKGBUILD mentions openssl.
        let _ = run([
            "convert".into(),
            "-y".into(),
            write_hello_deb().to_string_lossy().into(),
            "--workdir".into(),
            wd.to_string_lossy().into(),
        ]);
        let pb = std::fs::read_to_string(wd.join("PKGBUILD")).unwrap_or_default();
        assert!(
            pb.contains("openssl"),
            "needed_names must add openssl\n{pb}"
        );
        let _ = std::fs::remove_dir_all(wd);
        let _ = code;
    }

    #[test]
    #[ignore = "needs rpmbuild"]
    fn convert_rpm_archive_matches_payload() {
        if std::process::Command::new("rpmbuild")
            .arg("--version")
            .output()
            .is_err()
        {
            panic!("rpmbuild missing; keep #[ignore] on this test");
        }
        assert!(has_makepkg());
        let _l = crate::hooks::set_lookup(none_lookup());
        let _r = crate::hooks::set_resolver(Box::new(NoneRes));
        let dir = std::env::temp_dir().join(format!("packager-cvrpm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rpm = dir.join("hello-1.0-1.x86_64.rpm");
        crate::testpkg::write_rpm(
            &rpm,
            &crate::testpkg::RpmSpec {
                name: "hello".into(),
                version: "1.0".into(),
                release: "1".into(),
                arch: "x86_64".into(),
                requires: "glibc".into(),
                files: vec![("/usr/bin/hello".into(), b"hi\n".to_vec())],
                post: Some("ldconfig\n".into()),
            },
        )
        .unwrap();
        let wd = dir.join("wd");
        let code = run([
            "convert".into(),
            "-y".into(),
            rpm.to_string_lossy().into(),
            "--workdir".into(),
            wd.to_string_lossy().into(),
        ]);
        assert_eq!(code, 0);
        let pkg = std::fs::read_dir(&wd)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(".pkg.tar")
            })
            .expect("archive");
        let listing = String::from_utf8(
            std::process::Command::new("bsdtar")
                .args(["-tf", pkg.to_str().unwrap()])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let payload = wd.join("payload");
        let mut payload_files = Vec::new();
        collect_rel_files(&payload, &payload, &mut payload_files);
        for f in &payload_files {
            assert!(listing.contains(f), "archive missing {f}\n{listing}");
        }
        assert!(listing.contains("usr/bin/hello"), "{listing}");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn collect_rel_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                collect_rel_files(root, &path, out);
            } else {
                out.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
}
