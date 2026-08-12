//! Process-global test hooks with drop guards that restore on unwind.

use crate::depmap;
use crate::elfinfo;
use crate::error::Result;
use crate::ident;
use crate::lookup;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

pub type OwnedByFn = fn(&Path) -> Result<Option<String>>;
pub type ConfirmFn = fn(&str) -> bool;
pub type ScanFn = fn(&Path) -> Result<Vec<elfinfo::Info>>;
pub type UpgradeFn = fn(&Path, bool) -> Result<()>;

static OWNED_BY: Mutex<Option<OwnedByFn>> = Mutex::new(None);
pub static LOOKUP: Mutex<Option<lookup::Client>> = Mutex::new(None);
pub static MAP_RESOLVER: Mutex<Option<Box<dyn depmap::Resolver + Send + Sync>>> = Mutex::new(None);
pub static CONFIRM: Mutex<Option<ConfirmFn>> = Mutex::new(None);
static SCAN: Mutex<Option<ScanFn>> = Mutex::new(None);
static UPGRADE: Mutex<Option<UpgradeFn>> = Mutex::new(None);

/// Restores the previous hook value on drop, including during unwind.
#[must_use = "the hook is restored when this guard is dropped"]
pub struct Guard<T: 'static> {
    slot: &'static Mutex<Option<T>>,
    prev: Option<T>,
}

impl<T> Drop for Guard<T> {
    fn drop(&mut self) {
        let mut slot = match self.slot.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        *slot = self.prev.take();
    }
}

/// Swap `slot` to `next`; the returned guard restores the previous value.
pub fn set<T: 'static>(slot: &'static Mutex<Option<T>>, next: T) -> Guard<T> {
    let mut g = match slot.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let prev = g.replace(next);
    Guard { slot, prev }
}

pub fn set_owned_by(f: OwnedByFn) -> Guard<OwnedByFn> {
    set(&OWNED_BY, f)
}

pub fn set_lookup(c: lookup::Client) -> Guard<lookup::Client> {
    set(&LOOKUP, c)
}

pub fn set_resolver(
    r: Box<dyn depmap::Resolver + Send + Sync>,
) -> Guard<Box<dyn depmap::Resolver + Send + Sync>> {
    set(&MAP_RESOLVER, r)
}

pub fn set_confirm(f: ConfirmFn) -> Guard<ConfirmFn> {
    set(&CONFIRM, f)
}

pub fn set_scan(f: ScanFn) -> Guard<ScanFn> {
    set(&SCAN, f)
}

pub fn set_upgrade(f: UpgradeFn) -> Guard<UpgradeFn> {
    set(&UPGRADE, f)
}

pub(crate) fn owned_by_hook() -> Option<OwnedByFn> {
    match OWNED_BY.lock() {
        Ok(g) => *g,
        Err(p) => *p.into_inner(),
    }
}

fn lock_slot<T>(slot: &Mutex<Option<T>>) -> std::sync::MutexGuard<'_, Option<T>> {
    match slot.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Hooked lookup client, or extra=`pacman_si` + AUR=`live_aur`.
pub fn lookup_client() -> lookup::Client {
    match lock_slot(&LOOKUP).as_ref() {
        Some(c) => lookup::Client {
            extra: c.extra,
            aur: c.aur,
        },
        None => lookup::Client {
            extra: lookup::pacman_si,
            aur: lookup::live_aur,
        },
    }
}

/// Run `f` with the hooked resolver, or [`depmap::PkgfileResolver`].
pub fn with_resolver<R>(f: impl FnOnce(&dyn depmap::Resolver) -> R) -> R {
    match lock_slot(&MAP_RESOLVER).as_deref() {
        Some(r) => f(r),
        None => f(&depmap::PkgfileResolver),
    }
}

/// Hooked confirm, or stdin: empty / `y` / `yes` proceeds.
pub fn confirm(prompt: &str) -> bool {
    if let Some(f) = *lock_slot(&CONFIRM) {
        return f(prompt);
    }
    use std::io::{self, Write};
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(_) => {
            let t = line.trim();
            t.is_empty() || t.eq_ignore_ascii_case("y") || t.eq_ignore_ascii_case("yes")
        }
        Err(_) => false,
    }
}

/// Hooked ELF scan, or [`elfinfo::scan`].
pub fn scan(root: &Path) -> Result<Vec<elfinfo::Info>> {
    match *lock_slot(&SCAN) {
        Some(f) => f(root),
        None => elfinfo::scan(root),
    }
}

/// Hooked `pacman -U`, or [`crate::pm::upgrade`].
pub fn upgrade(pkg_path: &Path, noconfirm: bool) -> Result<()> {
    match *lock_slot(&UPGRADE) {
        Some(f) => f(pkg_path, noconfirm),
        None => crate::pm::upgrade(pkg_path, noconfirm),
    }
}

/// Host architecture from `uname -m`, mapped via [`ident::map_arch`].
pub fn host_arch() -> ident::Arch {
    let out = Command::new("uname").arg("-m").output().expect("uname -m");
    let raw = String::from_utf8_lossy(&out.stdout);
    ident::map_arch(raw.trim()).expect("unsupported host architecture")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn guard_restores_on_unwind() {
        static LAST: AtomicUsize = AtomicUsize::new(0);
        fn first(_: &Path) -> Result<Option<String>> {
            LAST.store(1, Ordering::SeqCst);
            Ok(Some("first".into()))
        }
        fn second(_: &Path) -> Result<Option<String>> {
            LAST.store(2, Ordering::SeqCst);
            Ok(Some("second".into()))
        }
        let _outer = set_owned_by(first);
        let panicked = std::panic::catch_unwind(|| {
            let _inner = set_owned_by(second);
            let _ = crate::pm::owned_by(Path::new("usr/bin/hello")).unwrap();
            assert_eq!(LAST.load(Ordering::SeqCst), 2);
            panic!("boom");
        });
        assert!(panicked.is_err());
        let got = crate::pm::owned_by(Path::new("usr/bin/hello")).unwrap();
        assert_eq!(got.as_deref(), Some("first"));
        assert_eq!(LAST.load(Ordering::SeqCst), 1);
    }
}
