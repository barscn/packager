//! Map Debian/RPM dependency names to Arch packages.

use std::collections::HashSet;
use std::process::Command;

/// Bucketed Arch package names after mapping.
#[derive(Clone, Debug, Default)]
pub struct Buckets {
    pub extra: Vec<String>,
    pub aur: Vec<String>,
    pub unmapped: Vec<String>,
}

/// Resolve a stripped dependency or soname to `(pkg, repo)`.
/// `repo` is one of `extra`, `multilib`, or `aur`.
pub trait Resolver {
    fn which(&self, name: &str) -> Option<(String, String)>;
}

/// Strip version constraints and RPM capability suffixes.
/// `libc6 (>= 2.28)` → `libc6`; `libc.so.6()(64bit)` → `libc.so.6`.
pub fn strip_constraint(raw: &str) -> String {
    let mut t = raw.trim().to_string();
    if t.is_empty() {
        return t;
    }
    // strip [arch]
    if let Some(i) = t.find('[') {
        if t.ends_with(']') {
            t.truncate(i);
            t = t.trim_end().to_string();
        }
    }
    // strip (constraint) / ()(64bit) etc.
    if let Some(i) = t.find('(') {
        if t.contains(')') {
            t.truncate(i);
            t = t.trim_end().to_string();
        }
    }
    t
}

/// Static Debian/RPM → Arch name table. Hits go in `Buckets.extra` and win over `Resolver`.
fn table_lookup(name: &str) -> Option<&'static str> {
    match name {
        "libc6" => Some("glibc"),
        "libgtk-3-0" => Some("gtk3"),
        "libssl3" => Some("openssl"),
        "zlib1g" => Some("zlib"),
        "libx11-6" => Some("libx11"),
        _ => None,
    }
}

fn should_skip(raw: &str, stripped: &str) -> bool {
    if stripped.is_empty() {
        return true;
    }
    let raw = raw.trim();
    if raw.starts_with("rpmlib(") {
        return true;
    }
    if stripped.starts_with("ld-linux") {
        return true;
    }
    false
}

/// Map raw depend names: strip → table → resolver. Dedup. Skip `rpmlib(*)` and `ld-linux*`.
pub fn map_names(raw: &[String], r: &dyn Resolver) -> Buckets {
    let mut buckets = Buckets::default();
    let mut seen = HashSet::new();

    for dep in raw {
        let stripped = strip_constraint(dep);
        if should_skip(dep, &stripped) {
            continue;
        }

        if let Some(arch) = table_lookup(&stripped) {
            if seen.insert(arch.to_string()) {
                buckets.extra.push(arch.to_string());
            }
            continue;
        }

        match r.which(&stripped) {
            Some((pkg, repo)) if repo == "aur" => {
                if seen.insert(pkg.clone()) {
                    buckets.aur.push(pkg);
                }
            }
            Some((pkg, repo)) if repo == "extra" || repo == "multilib" => {
                if seen.insert(pkg.clone()) {
                    buckets.extra.push(pkg);
                }
            }
            Some(_) | None => {
                // Unmapped names stay as the stripped original; never in extra.
                if seen.insert(stripped.clone()) {
                    buckets.unmapped.push(stripped);
                }
            }
        }
    }

    buckets
}

/// Resolve ELF `NEEDED` sonames via `Resolver` to Arch package names (extra/multilib only).
pub fn needed_names(sonames: &[String], r: &dyn Resolver) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for raw in sonames {
        let name = strip_constraint(raw);
        if should_skip(raw, &name) {
            continue;
        }
        if let Some((pkg, repo)) = r.which(&name) {
            if (repo == "extra" || repo == "multilib") && seen.insert(pkg.clone()) {
                out.push(pkg);
            }
        }
    }
    out
}

/// Resolve sonames with `pkgfile -b -v`.
#[derive(Clone, Debug, Default)]
pub struct PkgfileResolver;

impl Resolver for PkgfileResolver {
    fn which(&self, name: &str) -> Option<(String, String)> {
        let out = Command::new("pkgfile")
            .args(["-b", "-v", "--", name])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if let Some(hit) = parse_pkgfile_line(line) {
                return Some(hit);
            }
        }
        None
    }
}

/// Parse `repo/pkg ver\tpath` → `(pkg, repo)` with core→extra normalization.
fn parse_pkgfile_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let first = line.split_whitespace().next()?;
    let (repo, pkg) = first.split_once('/')?;
    if pkg.is_empty() {
        return None;
    }
    let repo = match repo {
        "core" | "extra" | "community" => "extra",
        "multilib" => "multilib",
        other => other,
    };
    Some((pkg.to_string(), repo.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Fake(HashMap<String, (String, String)>);
    impl Resolver for Fake {
        fn which(&self, name: &str) -> Option<(String, String)> {
            self.0.get(name).cloned()
        }
    }

    #[test]
    fn map_names_buckets() {
        let r = Fake(
            [(
                "some-aur-tool".into(),
                ("some-aur-tool".into(), "aur".into()),
            )]
            .into(),
        );
        let raw = [
            "libc6 (>= 2.28)",
            "libgtk-3-0",
            "some-aur-tool",
            "not-a-real-pkg",
            "rpmlib(CompressedFileNames)",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
        let b = map_names(&raw, &r);
        assert!(b.extra.iter().any(|s| s == "glibc"), "{:?}", b.extra);
        assert!(b.extra.iter().any(|s| s == "gtk3"), "{:?}", b.extra);
        assert!(b.aur.iter().any(|s| s == "some-aur-tool"), "{:?}", b.aur);
        assert!(
            b.unmapped.iter().any(|s| s == "not-a-real-pkg"),
            "{:?}",
            b.unmapped
        );
        assert!(!b.unmapped.iter().any(|s| s.contains("rpmlib")));
        assert!(!b.extra.iter().any(|s| s.contains("rpmlib")));
    }

    #[test]
    fn strip_constraint_examples() {
        assert_eq!(strip_constraint("libc6 (>= 2.28)"), "libc6");
        assert_eq!(strip_constraint("libc.so.6()(64bit)"), "libc.so.6");
    }

    #[test]
    fn needed_names_via_fake() {
        let r = Fake([("libc.so.6".into(), ("glibc".into(), "extra".into()))].into());
        let names = needed_names(&["libc.so.6".into()], &r);
        assert_eq!(names, ["glibc"]);
    }

    #[test]
    fn map_names_skips_ld_linux_and_dedups() {
        let r = Fake(HashMap::new());
        let raw = [
            "libc6",
            "libc6 (>= 2.31)",
            "ld-linux-x86-64.so.2",
            "ld-linux.so.2",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
        let b = map_names(&raw, &r);
        assert_eq!(b.extra, ["glibc"]);
        assert!(b.unmapped.is_empty(), "{:?}", b.unmapped);
        assert!(b.aur.is_empty());
    }

    #[test]
    fn parse_pkgfile_core_to_extra() {
        let hit = parse_pkgfile_line("core/glibc 2.40-1\t/usr/lib/libc.so.6").unwrap();
        assert_eq!(hit, ("glibc".into(), "extra".into()));
        let hit = parse_pkgfile_line("multilib/lib32-glibc 2.40-1 /usr/lib32/libc.so.6").unwrap();
        assert_eq!(hit, ("lib32-glibc".into(), "multilib".into()));
    }
}
