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
    format_report_with(r, false)
}

pub fn format_report_colored(r: &Report) -> String {
    format_report_with(r, true)
}

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD_RED: &str = "\x1b[1;31m";
const BOLD_YELLOW: &str = "\x1b[1;33m";
const BOLD_GREEN: &str = "\x1b[1;32m";
const YELLOW: &str = "\x1b[33m";

fn paint(on: bool, code: &str, s: &str) -> String {
    if on {
        format!("{code}{s}{RESET}")
    } else {
        s.to_string()
    }
}

fn value_sgr(r: &Report, key: &str) -> Option<&'static str> {
    match key {
        "verdict" => Some(match r.verdict {
            Verdict::Blocked => BOLD_RED,
            Verdict::ProceedWarnings => BOLD_YELLOW,
            Verdict::Proceed => BOLD_GREEN,
        }),
        "native" if r.lookup_err.is_some() => Some(BOLD_RED),
        "native" if !r.native.is_empty() => Some(BOLD_YELLOW),
        "unmapped" | "scripts" | "layout" | "conflicts" => Some(YELLOW),
        _ => None,
    }
}

fn format_report_with(r: &Report, color: bool) -> String {
    let rows = report_rows(r);
    let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let mut out = String::new();
    if let Some((key, value)) = rows.first() {
        out.push_str(&align_line(r, key, value, width, color));
        out.push('\n');
        out.push('\n');
    }
    for (key, value) in rows.iter().skip(1) {
        out.push_str(&align_line(r, key, value, width, color));
        out.push('\n');
    }
    out
}

fn report_rows(r: &Report) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    rows.push(("verdict", verdict_text(r)));
    rows.push(("source", format!("{}  ({})", r.source, r.format.as_str())));
    let arch_s = r.arch.map(|a| a.as_str()).unwrap_or("unknown");
    rows.push((
        "pkgname",
        format!("{}  {}-{}  {}", r.pkgname, r.pkgver, r.pkgrel, arch_s),
    ));
    rows.push(("native", native_text(r)));

    let mut deps = r.depends.extra.clone();
    deps.extend(r.depends.aur.iter().cloned());
    if !deps.is_empty() {
        rows.push(("depends", deps.join(", ")));
    }
    if !r.depends.unmapped.is_empty() {
        rows.push(("unmapped", r.depends.unmapped.join(", ")));
    }
    if !r.scripts.is_empty() {
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
        rows.push((
            "scripts",
            format!("{} {} — {}", r.scripts.len(), parts.join(", "), fate),
        ));
    }
    if !r.layout.is_empty() {
        rows.push(("layout", r.layout.join("; ")));
    }
    if !r.conflicts.is_empty() {
        rows.push(("conflicts", r.conflicts.join(", ")));
    }
    rows
}

fn verdict_text(r: &Report) -> String {
    match r.verdict {
        Verdict::Proceed => "proceed".to_string(),
        Verdict::ProceedWarnings => "proceed with warnings".to_string(),
        Verdict::Blocked => {
            if r.block_reason.is_empty() {
                "blocked".to_string()
            } else {
                format!("blocked ({})", r.block_reason)
            }
        }
    }
}

fn native_text(r: &Report) -> String {
    if r.lookup_err.is_some() {
        "lookup failed".into()
    } else if !r.native.is_empty() {
        r.native
            .iter()
            .map(|h| format!("{}/{} {}", h.repo, h.name, h.version))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        "no extra/AUR match".into()
    }
}

fn align_line(r: &Report, key: &str, value: &str, width: usize, color: bool) -> String {
    let pad = width + 2 - key.len() - 1;
    let label = paint(color, DIM, &format!("{key}:"));
    let value = match value_sgr(r, key) {
        Some(code) => paint(color, code, value),
        None => value.to_string(),
    };
    format!("{label}{}{value}", " ".repeat(pad))
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

    fn first_content_line(text: &str) -> &str {
        text.lines().find(|l| !l.is_empty()).unwrap_or("")
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
            "verdict:",
        ] {
            assert!(text.contains(prefix), "missing {prefix} in\n{text}");
        }
        assert!(!text.contains("scripts:"), "{text}");
        assert!(!text.contains("layout:"), "{text}");
        assert!(!text.contains("conflicts:"), "{text}");
        assert!(!text.contains("(none)"), "{text}");
    }

    #[test]
    fn format_omits_empty_optional_fields() {
        let text = format_report(&evaluate(base_in(&base_pkg())));
        assert!(text.contains("verdict:"));
        assert!(text.contains("source:"));
        assert!(text.contains("pkgname:"));
        assert!(text.contains("native:"));
        assert!(!text.contains("depends:"), "{text}");
        assert!(!text.contains("unmapped:"), "{text}");
        assert!(!text.contains("scripts:"), "{text}");
        assert!(!text.contains("layout:"), "{text}");
        assert!(!text.contains("conflicts:"), "{text}");
        assert!(!text.contains("(none)"), "{text}");
    }

    #[test]
    fn format_verdict_first_then_blank_then_source() {
        let text = format_report(&evaluate(base_in(&base_pkg())));
        assert!(first_content_line(&text).starts_with("verdict:"), "{text}");
        let mut lines = text.lines();
        assert!(lines.next().unwrap().starts_with("verdict:"));
        assert_eq!(lines.next(), Some(""));
        assert!(lines.next().unwrap().starts_with("source:"));
    }

    #[test]
    fn format_aligns_values() {
        let text = format_report(&evaluate(base_in(&base_pkg())));
        let expected = "\
verdict: proceed\n\
\n\
source:  foo.deb  (deb)\n\
pkgname: foo  1.0-1  x86_64\n\
native:  no extra/AUR match\n";
        assert_eq!(text, expected);
    }

    #[test]
    fn format_report_is_colorless() {
        let text = format_report(&evaluate(base_in(&base_pkg())));
        assert!(!text.contains('\u{1b}'), "{text:?}");
    }

    #[test]
    fn format_lookup_fail_does_not_claim_no_native() {
        let p = base_pkg();
        let mut inn = base_in(&p);
        inn.lookup_err = Some("offline".into());
        let text = format_report(&evaluate(inn));
        assert!(text.contains("native:"), "{text}");
        assert!(text.contains("lookup failed"), "{text}");
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

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c == '\u{1b}' && it.peek() == Some(&'[') {
                it.next();
                for x in it.by_ref() {
                    if x.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn format_colored_blocked_uses_sgr_and_strips_to_plain() {
        let p = base_pkg();
        let mut inn = base_in(&p);
        inn.native = vec![lookup::Hit {
            name: "foo".into(),
            version: "2".into(),
            repo: "extra".into(),
        }];
        let r = evaluate(inn);
        let plain = format_report(&r);
        let colored = format_report_colored(&r);
        assert!(colored.contains('\u{1b}'), "{colored:?}");
        assert!(
            colored.contains("31"),
            "blocked should be red:\n{colored:?}"
        );
        assert!(
            colored.contains("2m") || colored.contains("[2m"),
            "labels dim:\n{colored:?}"
        );
        assert_eq!(strip_ansi(&colored), plain);
        assert!(!plain.contains('\u{1b}'));
    }

    #[test]
    fn format_colored_lookup_fail_keeps_wording() {
        let p = base_pkg();
        let mut inn = base_in(&p);
        inn.lookup_err = Some("offline".into());
        let r = evaluate(inn);
        let colored = format_report_colored(&r);
        let text = strip_ansi(&colored);
        assert!(
            text.contains("native:") && text.contains("lookup failed"),
            "{text}"
        );
        assert!(!text.contains("no extra/AUR match"), "{text}");
        assert!(!text.contains("no native package"), "{text}");
    }
}
