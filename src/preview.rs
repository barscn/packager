//! Preview report and stop/warn/proceed verdict.

use crate::depmap;
use crate::elfinfo;
use crate::ident;
use crate::lookup;
use crate::source;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Proceed,
    ProceedWarnings,
    Blocked,
}

pub struct Report {
    pub source: String,
    pub format: ident::Format,
    pub pkgname: String,
    pub pkgver: String,
    pub pkgrel: String,
    pub epoch: String,
    pub arch: Option<ident::Arch>,
    pub native: Vec<lookup::Hit>,
    pub lookup_err: Option<String>,
    pub depends: depmap::Buckets,
    pub scripts: Vec<source::Script>,
    pub allow_scripts: bool,
    pub layout: Vec<String>,
    pub conflicts: Vec<String>,
    pub verdict: Verdict,
    pub block_reason: String,
}

pub struct Input<'a> {
    pub pkg: &'a source::Package,
    pub force: bool,
    pub allow_scripts: bool,
    pub host_arch: ident::Arch,
    pub native: Vec<lookup::Hit>,
    pub lookup_err: Option<String>,
    pub depends: depmap::Buckets,
    pub elf: Vec<elfinfo::Info>,
    pub file_owned_by: Box<dyn Fn(&str) -> Option<String>>, // rel path → owner pkg
}

pub fn evaluate(in_: Input<'_>) -> Report {
    let pkg = in_.pkg;
    let pkgname = ident::normalize_name(&pkg.raw_name);
    let pkgver = ident::normalize_ver(&pkg.raw_version);
    let pkgrel = ident::PKGREL.to_string();
    let epoch = pkg.epoch.clone();
    let arch = ident::map_arch(&pkg.raw_arch).ok();

    let layout = elfinfo::layout_warnings(&pkg.file_list, &in_.elf, in_.host_arch);

    let mut conflicts = Vec::new();
    for path in &pkg.file_list {
        if let Some(owner) = (in_.file_owned_by)(path) {
            conflicts.push(format!("{path} owned by {owner}"));
        }
    }

    let mut verdict = Verdict::Proceed;
    let mut block_reason = String::new();

    if arch.is_none() {
        // Wrong arch always blocks; --force does not override.
        verdict = Verdict::Blocked;
        block_reason = "wrong arch".into();
    } else if in_.lookup_err.is_some() {
        if in_.force {
            verdict = Verdict::ProceedWarnings;
        } else {
            verdict = Verdict::Blocked;
            block_reason = "lookup failed".into();
        }
    } else if !in_.native.is_empty() {
        if in_.force {
            verdict = Verdict::ProceedWarnings;
        } else {
            verdict = Verdict::Blocked;
            block_reason = "native package exists".into();
        }
    } else if !conflicts.is_empty() {
        if in_.force {
            verdict = Verdict::ProceedWarnings;
        } else {
            verdict = Verdict::Blocked;
            block_reason = "file conflict".into();
        }
    } else {
        let warn =
            !in_.depends.unmapped.is_empty() || !pkg.scripts.is_empty() || !layout.is_empty();
        if warn {
            verdict = Verdict::ProceedWarnings;
        }
    }

    Report {
        source: pkg.path.display().to_string(),
        format: pkg.format,
        pkgname,
        pkgver,
        pkgrel,
        epoch,
        arch,
        native: in_.native,
        lookup_err: in_.lookup_err,
        depends: in_.depends,
        scripts: pkg.scripts.clone(),
        allow_scripts: in_.allow_scripts,
        layout,
        conflicts,
        verdict,
        block_reason,
    }
}

/// First non-comment, non-empty token from a script body (skips shebang/comments).
fn first_command_token(body: &str) -> Option<&str> {
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        return line.split_whitespace().next();
    }
    None
}

pub fn format_report(r: &Report) -> String {
    let mut out = String::new();

    out.push_str(&format!("source: {}  ({})\n", r.source, r.format.as_str()));

    let arch_s = r.arch.map(|a| a.as_str()).unwrap_or("unknown");
    out.push_str(&format!(
        "pkgname: {}  {}-{}  {}\n",
        r.pkgname, r.pkgver, r.pkgrel, arch_s
    ));

    if r.lookup_err.is_some() {
        out.push_str("native: lookup failed\n");
    } else if !r.native.is_empty() {
        let parts: Vec<String> = r
            .native
            .iter()
            .map(|h| format!("{}/{} {}", h.repo, h.name, h.version))
            .collect();
        out.push_str(&format!("native: {}\n", parts.join(", ")));
    } else {
        out.push_str("native: no extra/AUR match\n");
    }

    let mut deps = r.depends.extra.clone();
    deps.extend(r.depends.aur.iter().cloned());
    if deps.is_empty() {
        out.push_str("depends: (none)\n");
    } else {
        out.push_str(&format!("depends: {}\n", deps.join(", ")));
    }

    if r.depends.unmapped.is_empty() {
        out.push_str("unmapped: (none)\n");
    } else {
        out.push_str(&format!("unmapped: {}\n", r.depends.unmapped.join(", ")));
    }

    if r.scripts.is_empty() {
        out.push_str("scripts: (none)\n");
    } else {
        let fate = if r.allow_scripts {
            "will become .install"
        } else {
            "will not be packaged"
        };
        let parts: Vec<String> = r
            .scripts
            .iter()
            .map(|s| {
                let cmd = first_command_token(&s.body).unwrap_or("?");
                format!("{} ({})", s.name, cmd)
            })
            .collect();
        out.push_str(&format!(
            "scripts: {} {} — {}\n",
            r.scripts.len(),
            parts.join(", "),
            fate
        ));
    }

    if r.layout.is_empty() {
        out.push_str("layout: (none)\n");
    } else {
        out.push_str(&format!("layout: {}\n", r.layout.join("; ")));
    }

    if r.conflicts.is_empty() {
        out.push_str("conflicts: (none)\n");
    } else {
        out.push_str(&format!("conflicts: {}\n", r.conflicts.join(", ")));
    }

    let v = match r.verdict {
        Verdict::Proceed => "proceed".to_string(),
        Verdict::ProceedWarnings => "proceed with warnings".to_string(),
        Verdict::Blocked => {
            if r.block_reason.is_empty() {
                "blocked".to_string()
            } else {
                format!("blocked ({})", r.block_reason)
            }
        }
    };
    out.push_str(&format!("verdict: {v}\n"));

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_pkg() -> source::Package {
        source::Package {
            format: ident::Format::Deb,
            path: "foo.deb".into(),
            raw_name: "foo".into(),
            raw_version: "1.0".into(),
            raw_arch: "amd64".into(),
            file_list: vec!["usr/bin/foo".into()],
            ..Default::default()
        }
    }

    fn base_in(p: &source::Package) -> Input<'_> {
        Input {
            pkg: p,
            force: false,
            allow_scripts: false,
            host_arch: ident::Arch::X86_64,
            native: vec![],
            lookup_err: None,
            depends: Default::default(),
            elf: vec![],
            file_owned_by: Box::new(|_| None),
        }
    }

    #[test]
    fn proceed() {
        let p = base_pkg();
        assert_eq!(evaluate(base_in(&p)).verdict, Verdict::Proceed);
    }

    #[test]
    fn native_stop_and_force() {
        let p = base_pkg();
        let mut inn = base_in(&p);
        inn.native = vec![lookup::Hit {
            name: "foo".into(),
            version: "2".into(),
            repo: "extra".into(),
        }];
        let r = evaluate(inn);
        assert_eq!(r.verdict, Verdict::Blocked);
        assert!(r.block_reason.contains("native package exists"));
        let mut inn = base_in(&p);
        inn.native = vec![lookup::Hit {
            name: "foo".into(),
            version: "2".into(),
            repo: "extra".into(),
        }];
        inn.force = true;
        assert_ne!(evaluate(inn).verdict, Verdict::Blocked);
    }

    #[test]
    fn lookup_fail() {
        let p = base_pkg();
        let mut inn = base_in(&p);
        inn.lookup_err = Some("offline".into());
        let r = evaluate(inn);
        assert_eq!(r.verdict, Verdict::Blocked);
        assert!(r.block_reason.contains("lookup failed"));
    }

    #[test]
    fn conflict() {
        let p = base_pkg();
        let mut inn = base_in(&p);
        inn.file_owned_by = Box::new(|_| Some("other".into()));
        let r = evaluate(inn);
        assert_eq!(r.verdict, Verdict::Blocked);
        assert!(r.block_reason.contains("file conflict"));
    }

    #[test]
    fn scripts_warn() {
        let mut p = base_pkg();
        p.scripts = vec![source::Script {
            name: "postinst".into(),
            body: "true".into(),
        }];
        assert_eq!(evaluate(base_in(&p)).verdict, Verdict::ProceedWarnings);
    }

    #[test]
    fn wrong_arch_ignores_force() {
        let mut p = base_pkg();
        p.raw_arch = "i386".into();
        let mut inn = base_in(&p);
        inn.force = true;
        let r = evaluate(inn);
        assert_eq!(r.verdict, Verdict::Blocked);
        assert!(r.block_reason.contains("wrong arch"));
    }

    #[test]
    fn layout_warn() {
        let mut p = base_pkg();
        p.file_list = vec!["usr/lib/x86_64-linux-gnu/x.so".into()];
        let r = evaluate(base_in(&p));
        assert!(
            r.layout.iter().any(|w| w.contains("Debian multiarch path")),
            "{:?}",
            r.layout
        );
        assert_ne!(r.verdict, Verdict::Blocked);
    }

    #[test]
    fn format_fields() {
        let p = base_pkg();
        let mut inn = base_in(&p);
        inn.depends.extra = vec!["glibc".into()];
        inn.depends.unmapped = vec!["libfoo-dev".into()];
        let text = format_report(&evaluate(inn));
        for prefix in [
            "source:",
            "pkgname:",
            "native:",
            "depends:",
            "unmapped:",
            "scripts:",
            "layout:",
            "conflicts:",
            "verdict:",
        ] {
            assert!(text.contains(prefix), "missing {prefix} in\n{text}");
        }
    }

    #[test]
    fn format_lookup_fail_does_not_claim_no_native() {
        let p = base_pkg();
        let mut inn = base_in(&p);
        inn.lookup_err = Some("offline".into());
        let text = format_report(&evaluate(inn));
        assert!(text.contains("native: lookup failed"), "{text}");
        assert!(!text.contains("no extra/AUR match"), "{text}");
        assert!(!text.contains("no native package"), "{text}");
    }

    #[test]
    fn format_scripts_summarize_command() {
        let mut p = base_pkg();
        p.scripts = vec![source::Script {
            name: "postinst".into(),
            body: "#!/bin/sh\nupdate-desktop-database\n".into(),
        }];
        let text = format_report(&evaluate(base_in(&p)));
        assert!(text.contains("update-desktop-database"), "{text}");
        assert!(text.contains("will not be packaged"), "{text}");
    }
}
