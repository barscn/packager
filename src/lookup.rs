//! Extra / multilib via local pacman sync DB; AUR via RPC; static name aliases.

use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::ident::normalize_name;

/// A name that exists in extra, multilib, or the AUR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub name: String,
    pub version: String,
    pub repo: String, // extra | multilib | aur (or core from pacman -Si)
}

pub type ExtraFn = fn(&str) -> Result<Option<Hit>>;
pub type AurFn = fn(&str) -> Result<Option<Hit>>;

pub struct Client {
    pub extra: ExtraFn,
    pub aur: AurFn,
}

impl Client {
    /// For each name: extra first, else AUR. Any `Err` aborts the whole find.
    pub fn find(&self, names: &[String]) -> Result<Vec<Hit>> {
        let mut hits = Vec::new();
        for name in names {
            if let Some(h) = (self.extra)(name)? {
                hits.push(h);
                continue;
            }
            if let Some(h) = (self.aur)(name)? {
                hits.push(h);
            }
        }
        Ok(hits)
    }
}

/// Normalized pkgname/raw plus static aliases, deduped, stable order.
pub fn candidates(pkgname: &str, raw_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for seed in [pkgname, raw_name] {
        let n = normalize_name(seed);
        if n.is_empty() {
            continue;
        }
        push_unique(&mut out, n.clone());
        for a in alias(&n) {
            push_unique(&mut out, a);
        }
    }
    out
}

/// Static alias targets for a normalized package name.
pub fn alias(name: &str) -> Vec<String> {
    match name {
        "google-chrome-stable" => vec!["google-chrome".into()],
        _ => Vec::new(),
    }
}

fn push_unique(out: &mut Vec<String>, s: String) {
    if !out.iter().any(|x| x == &s) {
        out.push(s);
    }
}

#[derive(Debug, Deserialize)]
struct AurRpc {
    resultcount: u64,
    results: Vec<AurResult>,
}

#[derive(Debug, Deserialize)]
struct AurResult {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Version")]
    version: String,
}

/// Parse AUR RPC v5 multiinfo body → first hit or None when resultcount is 0.
pub fn parse_aur(body: &[u8]) -> Result<Option<Hit>> {
    let rpc: AurRpc =
        serde_json::from_slice(body).map_err(|e| Error::msg(format!("AUR JSON: {e}")))?;
    if rpc.resultcount == 0 || rpc.results.is_empty() {
        return Ok(None);
    }
    let r = &rpc.results[0];
    Ok(Some(Hit {
        name: r.name.clone(),
        version: r.version.clone(),
        repo: "aur".into(),
    }))
}

/// Live AUR RPC: `https://aur.archlinux.org/rpc/v5/info?arg[]=`.
pub fn live_aur(name: &str) -> Result<Option<Hit>> {
    let url = format!("https://aur.archlinux.org/rpc/v5/info?arg[]={name}");
    let body = ureq::get(&url)
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| Error::msg(format!("AUR request failed: {e}")))?
        .into_string()
        .map_err(|e| Error::msg(format!("AUR body read failed: {e}")))?;
    parse_aur(body.as_bytes())
}

/// Query local sync DB via `pacman -Si`; parse Repository / Name / Version.
pub fn pacman_si(name: &str) -> Result<Option<Hit>> {
    let out = Command::new("pacman").args(["-Si", "--", name]).output()?;
    if !out.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(parse_pacman_si(&text))
}

fn parse_pacman_si(text: &str) -> Option<Hit> {
    let mut repo: Option<String> = None;
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    for line in text.lines() {
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim();
        match key {
            "Repository" => {
                if repo.is_some() && name.is_some() && version.is_some() {
                    break; // first package block only
                }
                repo = Some(val.to_string());
            }
            "Name" => name = Some(val.to_string()),
            "Version" => version = Some(val.to_string()),
            _ => {}
        }
    }
    match (repo, name, version) {
        (Some(repo), Some(name), Some(version)) => Some(Hit {
            name,
            version,
            repo,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_aliases() {
        let cs = candidates("google-chrome-stable", "google-chrome-stable");
        assert!(cs.iter().any(|s| s == "google-chrome"), "{cs:?}");
        assert!(cs.iter().any(|s| s == "google-chrome-stable"), "{cs:?}");
    }

    #[test]
    fn parse_aur_fixture() {
        let b = include_bytes!("lookup/testdata/aur_info.json");
        let h = parse_aur(b).unwrap().unwrap();
        assert_eq!(h.name, "yay");
        assert!(!h.version.is_empty());
        assert_eq!(h.repo, "aur");
    }

    #[test]
    fn find_lookup_error() {
        fn boom(_: &str) -> Result<Option<Hit>> {
            Err(Error::msg("offline"))
        }
        fn none(_: &str) -> Result<Option<Hit>> {
            Ok(None)
        }
        let c = Client {
            extra: boom,
            aur: none,
        };
        assert!(c.find(&["foo".into()]).is_err());
    }

    #[test]
    #[ignore = "set PACKAGER_LIVE_AUR=1 and run with --ignored"]
    fn live_aur_optional() {
        let h = live_aur("yay").unwrap().unwrap();
        assert_eq!(h.name, "yay");
    }

    #[test]
    fn pacman_si_real_sync_db() {
        let ok = std::process::Command::new("pacman")
            .args(["-Si", "pacman"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("no pacman sync db on this host; not failing");
            return;
        }
        let h = pacman_si("pacman")
            .unwrap()
            .expect("pacman must resolve in extra/core");
        assert!(h.repo == "core" || h.repo == "extra", "{h:?}");
        assert!(!h.version.is_empty());
    }
}
