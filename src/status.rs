//! Remind-later status text for tracked converted packages.

use crate::error::Result;
use crate::lookup;
use crate::state;

pub fn report(
    records: &[state::Record],
    q: &dyn Fn(&str) -> Result<Option<String>>,
    find: &dyn Fn(&[String]) -> Result<Vec<lookup::Hit>>,
) -> String {
    let mut out = String::new();
    for rec in records {
        let installed = match q(&rec.pkgname) {
            Ok(v) => v,
            Err(_) => {
                out.push_str(&format!(
                    "{}  {}  (from {})\n  query failed\n",
                    rec.pkgname, rec.pkgver, rec.source_name
                ));
                continue;
            }
        };
        let ver = installed.as_deref().unwrap_or(rec.pkgver.as_str());
        out.push_str(&format!(
            "{}  {}  (from {})\n",
            rec.pkgname, ver, rec.source_name
        ));
        if installed.is_none() {
            out.push_str(&format!(
                "  not installed — packager forget {}\n",
                rec.pkgname
            ));
            continue;
        }
        let installed_ver = installed.as_deref().unwrap();
        if !same_converted_version(installed_ver, &rec.pkgver) {
            out.push_str(&format!(
                "  replaced ({installed_ver}) — packager forget {}\n",
                rec.pkgname
            ));
            continue;
        }
        let names = lookup::candidates(&rec.pkgname, &rec.pkgname);
        match find(&names) {
            Err(_) => out.push_str("  lookup failed\n"),
            Ok(hits) if hits.is_empty() => out.push_str("  no native package\n"),
            Ok(hits) => {
                let h = &hits[0];
                if h.repo == "aur" {
                    out.push_str(&format!("  {} is in the AUR\n", h.name));
                } else {
                    out.push_str(&format!(
                        "  {} has {} {}  →  replace: sudo pacman -S {}\n",
                        h.repo, h.name, h.version, h.name
                    ));
                }
            }
        }
    }
    out
}

/// Recorded `pkgver` vs `pacman -Q` (`[epoch:]pkgver-pkgrel`).
fn same_converted_version(installed: &str, recorded: &str) -> bool {
    if installed == recorded {
        return true;
    }
    let inst = strip_epoch(installed);
    let rec = strip_epoch(recorded);
    if inst == rec {
        return true;
    }
    match inst.rsplit_once('-') {
        Some((ver, rel)) if !rel.is_empty() && ver == rec => true,
        _ => false,
    }
}

fn strip_epoch(v: &str) -> &str {
    v.split_once(':').map(|(_, rest)| rest).unwrap_or(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::run;
    use crate::error::Error;
    use crate::lookup;
    use crate::state;

    fn empty_rec(name: &str, ver: &str, src: &str) -> state::Record {
        state::Record {
            pkgname: name.into(),
            pkgver: ver.into(),
            arch: "x86_64".into(),
            source_name: src.into(),
            source_checksum: String::new(),
            format: "deb".into(),
            installed_at: String::new(),
            allow_scripts: false,
            forced: false,
            workdir: String::new(),
        }
    }

    #[test]
    fn report_native_extra() {
        let recs = [empty_rec("zoom", "6.2.0", "zoom_amd64.deb")];
        let out = report(&recs, &|_| Ok(Some("6.2.0-1".into())), &|_| {
            Ok(vec![lookup::Hit {
                name: "zoom".into(),
                version: "6.2.3-1".into(),
                repo: "extra".into(),
            }])
        });
        assert!(out.contains("replace: sudo pacman -S zoom"), "{out}");
    }

    #[test]
    fn report_native_aur() {
        let recs = [empty_rec("acme-vpn", "1.4", "acme-vpn.rpm")];
        let out = report(&recs, &|_| Ok(Some("1.4-1".into())), &|_| {
            Ok(vec![lookup::Hit {
                name: "acme-vpn".into(),
                version: "1.5-1".into(),
                repo: "aur".into(),
            }])
        });
        assert!(out.contains("acme-vpn is in the AUR"), "{out}");
        assert!(!out.contains("pacman -S"), "{out}");
    }

    #[test]
    fn report_no_native() {
        let recs = [empty_rec("acme-vpn", "1.4", "acme-vpn.rpm")];
        let out = report(&recs, &|_| Ok(Some("1.4-1".into())), &|_| Ok(vec![]));
        assert!(out.contains("no native package"), "{out}");
    }

    #[test]
    fn report_lookup_fail() {
        let recs = [empty_rec("acme-vpn", "1.4", "acme-vpn.rpm")];
        let out = report(&recs, &|_| Ok(Some("1.4-1".into())), &|_| {
            Err(Error::msg("offline"))
        });
        assert!(out.contains("lookup failed"), "{out}");
        assert!(!out.contains("no native package"), "{out}");
    }

    #[test]
    fn report_stale() {
        let recs = [empty_rec("gone", "1", "gone.deb")];
        let out = report(&recs, &|_| Ok(None), &|_| Ok(vec![]));
        assert!(out.contains("forget"), "{out}");
    }

    #[test]
    fn report_same_name_native_replace_is_stale() {
        let recs = [empty_rec("zoom", "6.2.0", "zoom_amd64.deb")];
        let out = report(&recs, &|_| Ok(Some("6.2.3-1".into())), &|_| {
            Ok(vec![lookup::Hit {
                name: "zoom".into(),
                version: "6.2.3-1".into(),
                repo: "extra".into(),
            }])
        });
        assert!(out.contains("forget"), "{out}");
        assert!(out.contains("replaced (6.2.3-1)"), "{out}");
        assert!(!out.contains("replace: sudo pacman -S"), "{out}");
    }

    #[test]
    fn report_matching_pkgrel_is_not_stale() {
        assert!(same_converted_version("6.2.0-1", "6.2.0"));
        assert!(same_converted_version("1:6.2.0-1", "6.2.0"));
        assert!(!same_converted_version("6.2.3-1", "6.2.0"));
    }

    #[test]
    fn forget_no_remove() {
        let dir = std::env::temp_dir().join(format!("packager-fg-{}", std::process::id()));
        let _g = crate::state::set_data_dir_for_test(Some(dir.clone()));
        crate::state::write(&empty_rec("hello", "1.0", "hello.deb")).unwrap();
        use std::sync::atomic::{AtomicBool, Ordering};
        static REMOVED: AtomicBool = AtomicBool::new(false);
        fn rem(_: &str, _: bool) -> crate::error::Result<()> {
            REMOVED.store(true, Ordering::SeqCst);
            Ok(())
        }
        let _r = crate::hooks::set_remove(rem);
        let _f = crate::hooks::set_forget_remove(|_| false);
        assert_eq!(run(["forget".into(), "hello".into()]), 0);
        assert!(!REMOVED.load(Ordering::SeqCst));
        assert!(crate::state::read("hello").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn forget_yes_flag_does_not_auto_remove() {
        let dir = std::env::temp_dir().join(format!("packager-fgy-{}", std::process::id()));
        let _g = crate::state::set_data_dir_for_test(Some(dir.clone()));
        crate::state::write(&empty_rec("hello", "1.0", "hello.deb")).unwrap();
        use std::sync::atomic::{AtomicUsize, Ordering};
        static ASKED: AtomicUsize = AtomicUsize::new(0);
        static REMOVED: AtomicUsize = AtomicUsize::new(0);
        fn rem(_: &str, _: bool) -> crate::error::Result<()> {
            REMOVED.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn ask(_: &str) -> bool {
            ASKED.fetch_add(1, Ordering::SeqCst);
            false
        }
        let _r = crate::hooks::set_remove(rem);
        let _f = crate::hooks::set_forget_remove(ask);
        assert_eq!(run(["forget".into(), "-y".into(), "hello".into()]), 0);
        assert!(
            ASKED.load(Ordering::SeqCst) >= 1,
            "-y must not skip the remove prompt"
        );
        assert_eq!(REMOVED.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn forget_remove_fail_keeps_json() {
        let dir = std::env::temp_dir().join(format!("packager-fgf-{}", std::process::id()));
        let _g = crate::state::set_data_dir_for_test(Some(dir.clone()));
        crate::state::write(&empty_rec("hello", "1.0", "hello.deb")).unwrap();
        fn rem(_: &str, _: bool) -> crate::error::Result<()> {
            Err(crate::error::Error::msg("busy"))
        }
        let _r = crate::hooks::set_remove(rem);
        let _f = crate::hooks::set_forget_remove(|_| true);
        assert_eq!(run(["forget".into(), "hello".into()]), 1);
        assert!(crate::state::read("hello").is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn forget_remove_ok() {
        let dir = std::env::temp_dir().join(format!("packager-fgo-{}", std::process::id()));
        let _g = crate::state::set_data_dir_for_test(Some(dir.clone()));
        crate::state::write(&empty_rec("hello", "1.0", "hello.deb")).unwrap();
        fn rem(_: &str, noconfirm: bool) -> crate::error::Result<()> {
            assert!(!noconfirm);
            Ok(())
        }
        let _r = crate::hooks::set_remove(rem);
        let _f = crate::hooks::set_forget_remove(|_| true);
        assert_eq!(run(["forget".into(), "hello".into()]), 0);
        assert!(crate::state::read("hello").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
