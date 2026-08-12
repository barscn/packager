//! Render PKGBUILD and optional `.install` from package metadata.

use crate::depmap;
use crate::error::Result;
use crate::ident;
use crate::source;
use std::collections::HashSet;
use std::path::PathBuf;

pub struct Options {
    pub allow_scripts: bool,
    pub workdir: PathBuf,
    pub payload_rel: String, // "payload"
}

/// Resolver that never resolves — table-only mapping via [`depmap::map_names`].
struct NoneRes;

impl depmap::Resolver for NoneRes {
    fn which(&self, _name: &str) -> Option<(String, String)> {
        None
    }
}

/// Write `workdir/PKGBUILD`, always `scripts.orig/`, and optional `.install`.
pub fn write(pkg: &source::Package, deps: &depmap::Buckets, opt: &Options) -> Result<()> {
    let pkgname = ident::normalize_name(&pkg.raw_name);
    let pkgver = ident::normalize_ver(&pkg.raw_version);
    let arch = ident::map_arch(&pkg.raw_arch)?;
    let pkgrel = ident::PKGREL;

    std::fs::create_dir_all(&opt.workdir)?;

    // Always preserve vendor scripts for inspection.
    if !pkg.scripts.is_empty() {
        let orig = opt.workdir.join("scripts.orig");
        std::fs::create_dir_all(&orig)?;
        for s in &pkg.scripts {
            std::fs::write(orig.join(&s.name), &s.body)?;
        }
    }

    let write_install = opt.allow_scripts && !pkg.scripts.is_empty();
    if write_install {
        let install_path = opt.workdir.join(format!("{pkgname}.install"));
        std::fs::write(&install_path, render_install(&pkg.scripts))?;
    }

    let provides = map_relation_names(&pkg.provides, &pkgname);
    let conflicts = map_relation_names(&pkg.conflicts, &pkgname);
    let replaces = map_relation_names(&pkg.replaces, &pkgname);

    let mut pb = String::new();
    pb.push_str(&format!("# Maintainer: {}\n", ident::PACKAGER_FIELD));
    pb.push_str(&format!("pkgname={pkgname}\n"));
    pb.push_str(&format!("pkgver={pkgver}\n"));
    pb.push_str(&format!("pkgrel={pkgrel}\n"));
    if epoch_emit(&pkg.epoch) {
        pb.push_str(&format!("epoch={}\n", pkg.epoch.trim()));
    }
    pb.push_str(&format!("arch=('{}')\n", arch.as_str()));
    pb.push_str(&format!("depends=({})\n", join_names(&deps.extra)));
    if !provides.is_empty() {
        pb.push_str(&format!("provides=({})\n", join_names(&provides)));
    }
    if !conflicts.is_empty() {
        pb.push_str(&format!("conflicts=({})\n", join_names(&conflicts)));
    }
    if !replaces.is_empty() {
        pb.push_str(&format!("replaces=({})\n", join_names(&replaces)));
    }
    if write_install {
        pb.push_str(&format!("install={pkgname}.install\n"));
    }
    pb.push_str(&format!("packager=\"{}\"\n", ident::PACKAGER_FIELD));
    pb.push_str("options=(!strip !docs !libtool !staticlibs emptydirs)\n");
    pb.push('\n');
    pb.push_str("package() {\n");
    pb.push_str(&format!(
        "  cp -a \"$startdir/{}/.\" \"$pkgdir/\"\n",
        opt.payload_rel
    ));
    pb.push_str("}\n");

    std::fs::write(opt.workdir.join("PKGBUILD"), pb)?;
    Ok(())
}

fn epoch_emit(epoch: &str) -> bool {
    let e = epoch.trim();
    !e.is_empty() && e != "0" && e != "(none)"
}

fn join_names(names: &[String]) -> String {
    names.join(" ")
}

/// Map relation names with the static table only; keep Extra hits and names equal to pkgname.
fn map_relation_names(raw: &[String], pkgname: &str) -> Vec<String> {
    let buckets = depmap::map_names(raw, &NoneRes);
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for name in buckets.extra {
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }

    // Unmapped/AUR-excluded: still emit if they normalize to this package's name.
    for name in raw {
        let stripped = depmap::strip_constraint(name);
        if stripped.is_empty() {
            continue;
        }
        let n = ident::normalize_name(&stripped);
        if n == pkgname && seen.insert(pkgname.to_string()) {
            out.push(pkgname.to_string());
        }
    }

    out
}

fn render_install(scripts: &[source::Script]) -> String {
    let mut out = String::new();
    for s in scripts {
        let targets: &[&str] = match s.name.as_str() {
            "postinst" | "post" => &["post_install", "post_upgrade"],
            "preinst" | "pre" => &["pre_install", "pre_upgrade"],
            "prerm" | "preun" => &["pre_remove"],
            "postrm" | "postun" => &["post_remove"],
            _ => continue,
        };
        for t in targets {
            out.push_str(t);
            out.push_str("() {\n");
            out.push_str(&s.body);
            if !s.body.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("}\n\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident;

    fn pkg() -> source::Package {
        source::Package {
            format: ident::Format::Deb,
            raw_name: "hello_app".into(),
            raw_version: "1.0".into(),
            raw_arch: "amd64".into(),
            scripts: vec![source::Script {
                name: "postinst".into(),
                body: "update-desktop-database\n".into(),
            }],
            ..Default::default()
        }
    }

    fn deps() -> depmap::Buckets {
        depmap::Buckets {
            extra: vec!["glibc".into(), "gtk3".into()],
            aur: vec!["foo-aur".into()],
            unmapped: vec!["libfoo-dev".into()],
        }
    }

    #[test]
    fn write_omits_unmapped_and_aur() {
        let wd = std::env::temp_dir().join(format!("packager-pb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wd);
        std::fs::create_dir_all(&wd).unwrap();
        write(
            &pkg(),
            &deps(),
            &Options {
                allow_scripts: false,
                workdir: wd.clone(),
                payload_rel: "payload".into(),
            },
        )
        .unwrap();
        let s = std::fs::read_to_string(wd.join("PKGBUILD")).unwrap();
        for want in [
            "pkgname=hello-app",
            "pkgrel=1",
            "arch=('x86_64')",
            "depends=(glibc gtk3)",
            r#"packager="packager <packager@local>""#,
        ] {
            assert!(s.contains(want), "missing {want} in\n{s}");
        }
        for ban in ["libfoo-dev", "foo-aur", "install="] {
            assert!(!s.contains(ban), "unexpected {ban} in\n{s}");
        }
        assert!(wd.join("scripts.orig/postinst").exists());
        let _ = std::fs::remove_dir_all(wd);
    }

    #[test]
    fn write_allow_scripts() {
        let wd = std::env::temp_dir().join(format!("packager-pbs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wd);
        std::fs::create_dir_all(&wd).unwrap();
        write(
            &pkg(),
            &deps(),
            &Options {
                allow_scripts: true,
                workdir: wd.clone(),
                payload_rel: "payload".into(),
            },
        )
        .unwrap();
        let s = std::fs::read_to_string(wd.join("PKGBUILD")).unwrap();
        assert!(s.contains("install=hello-app.install"), "{s}");
        let inst = std::fs::read_to_string(wd.join("hello-app.install")).unwrap();
        assert!(inst.contains("post_install()"), "{inst}");
        let _ = std::fs::remove_dir_all(wd);
    }

    #[test]
    fn write_maps_conflicts_provides() {
        let mut p = pkg();
        p.provides = vec!["hello_app".into(), "libfoo-dev".into()];
        p.conflicts = vec!["hello_app".into()];
        p.replaces = vec!["not-a-real-pkg".into()];
        let wd = std::env::temp_dir().join(format!("packager-pbc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wd);
        std::fs::create_dir_all(&wd).unwrap();
        write(
            &p,
            &deps(),
            &Options {
                allow_scripts: false,
                workdir: wd.clone(),
                payload_rel: "payload".into(),
            },
        )
        .unwrap();
        let s = std::fs::read_to_string(wd.join("PKGBUILD")).unwrap();
        assert!(
            s.contains("provides=(hello-app)") || s.contains("provides=('hello-app')"),
            "{s}"
        );
        assert!(
            s.contains("conflicts=(hello-app)") || s.contains("conflicts=('hello-app')"),
            "{s}"
        );
        assert!(!s.contains("libfoo-dev"), "{s}");
        assert!(!s.contains("not-a-real-pkg"), "{s}");
        let _ = std::fs::remove_dir_all(wd);
    }
}
