//! Process-global test hooks with drop guards that restore on unwind.

use crate::error::Result;
use crate::ident;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

pub type OwnedByFn = fn(&Path) -> Result<Option<String>>;

static OWNED_BY: Mutex<Option<OwnedByFn>> = Mutex::new(None);

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

pub(crate) fn owned_by_hook() -> Option<OwnedByFn> {
    match OWNED_BY.lock() {
        Ok(g) => *g,
        Err(p) => *p.into_inner(),
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
