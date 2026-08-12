use crate::error::{Error, Result};

/// Source package format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Deb,
    Rpm,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Deb => "deb",
            Format::Rpm => "rpm",
        }
    }
}

/// Target pacman architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

/// Always `1` for converted packages.
pub const PKGREL: &str = "1";

/// PKGBUILD `packager` field value.
pub const PACKAGER_FIELD: &str = "packager <packager@local>";

/// Lowercase and replace `_` with `-` (trim first).
pub fn normalize_name(raw: &str) -> String {
    raw.trim().to_lowercase().replace('_', "-")
}

/// Map epoch/version separators: `:` → `.`, `/` → `_`.
pub fn normalize_ver(raw: &str) -> String {
    raw.replace(':', ".").replace('/', "_")
}

/// Map Debian/RPM arch strings to pacman `Arch`.
pub fn map_arch(raw: &str) -> Result<Arch> {
    match raw.to_ascii_lowercase().as_str() {
        "amd64" | "x86_64" => Ok(Arch::X86_64),
        "arm64" | "aarch64" => Ok(Arch::Aarch64),
        _ => Err(Error::msg(format!("wrong arch: {raw}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_lowercases_underscores() {
        assert_eq!(
            normalize_name("Google_Chrome_Stable"),
            "google-chrome-stable"
        );
    }

    #[test]
    fn ver_replaces_illegal_chars() {
        assert_eq!(normalize_ver("1:2.3/4"), "1.2.3_4");
    }

    #[test]
    fn map_arch_ok() {
        assert_eq!(map_arch("amd64").unwrap(), Arch::X86_64);
        assert_eq!(map_arch("x86_64").unwrap(), Arch::X86_64);
        assert_eq!(map_arch("arm64").unwrap(), Arch::Aarch64);
        assert_eq!(map_arch("aarch64").unwrap(), Arch::Aarch64);
    }

    #[test]
    fn map_arch_bad() {
        let e = map_arch("i386").unwrap_err();
        assert!(e.to_string().contains("wrong arch"), "{e}");
    }
}
