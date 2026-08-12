//! CLI flags and subcommands (parsing only; pipeline later).

use crate::error::{Error, Result};
use std::path::PathBuf;
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
            let f = pos.next().ok_or_else(|| {
                Error::msg(format!("usage: packager {first} <file.deb|.rpm>"))
            })?;
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
        Ok(cfg) => match *HANDLE.lock().unwrap() {
            Some(f) => f(&cfg),
            None => 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
}
