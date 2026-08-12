#![cfg(feature = "system")]

use packager::cli::run;
use packager::state;
use packager::testpkg::{write_deb, DebSpec};
use std::process::Command;

fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| o.stdout == b"0\n")
        .unwrap_or(false)
}

fn fixture_deb() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("packager-sys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("packager-systest_1.0_amd64.deb");
    write_deb(
        &p,
        &DebSpec {
            name: "packager-systest".into(),
            version: "1.0".into(),
            arch: "amd64".into(),
            depends: String::new(),
            files: vec![(
                "./usr/share/packager-systest/README".into(),
                b"ok\n".to_vec(),
            )],
            postinst: None,
        },
    )
    .unwrap();
    p
}

#[test]
fn system_install_then_forget_without_remove() {
    if !is_root() {
        panic!("rerun as root: cargo test --features system --test system");
    }
    let data = std::env::temp_dir().join(format!("packager-sysd-{}", std::process::id()));
    state::set_data_dir_for_test(Some(data.clone()));
    let deb = fixture_deb();
    let wd = std::env::temp_dir().join(format!("packager-sysw-{}", std::process::id()));
    assert_eq!(
        run([
            "install".into(),
            "-y".into(),
            deb.to_string_lossy().into(),
            "--workdir".into(),
            wd.to_string_lossy().into()
        ]),
        0
    );
    assert!(Command::new("pacman")
        .args(["-Q", "packager-systest"])
        .status()
        .unwrap()
        .success());
    let _f = packager::hooks::set_forget_remove(|_| false);
    assert_eq!(run(["forget".into(), "packager-systest".into()]), 0);
    assert!(state::read("packager-systest").is_err());
    assert!(Command::new("pacman")
        .args(["-Q", "packager-systest"])
        .status()
        .unwrap()
        .success());
    let _ = Command::new("pacman")
        .args(["-R", "--noconfirm", "packager-systest"])
        .status();
    state::set_data_dir_for_test(None);
}

#[test]
fn system_forget_with_remove() {
    if !is_root() {
        panic!("rerun as root: cargo test --features system --test system");
    }
    let data = std::env::temp_dir().join(format!("packager-sysd2-{}", std::process::id()));
    state::set_data_dir_for_test(Some(data.clone()));
    let deb = fixture_deb();
    let wd = std::env::temp_dir().join(format!("packager-sysw2-{}", std::process::id()));
    assert_eq!(
        run([
            "install".into(),
            "-y".into(),
            deb.to_string_lossy().into(),
            "--workdir".into(),
            wd.to_string_lossy().into()
        ]),
        0
    );
    let _f = packager::hooks::set_forget_remove(|_| true);
    assert_eq!(run(["forget".into(), "packager-systest".into()]), 0);
    assert!(state::read("packager-systest").is_err());
    assert!(!Command::new("pacman")
        .args(["-Q", "packager-systest"])
        .status()
        .unwrap()
        .success());
    state::set_data_dir_for_test(None);
}

#[test]
fn system_native_stop() {
    if !is_root() {
        panic!("rerun as root: cargo test --features system --test system");
    }
    fn hit(_: &str) -> packager::error::Result<Option<packager::lookup::Hit>> {
        Ok(Some(packager::lookup::Hit {
            name: "packager-systest".into(),
            version: "9".into(),
            repo: "extra".into(),
        }))
    }
    fn n(_: &str) -> packager::error::Result<Option<packager::lookup::Hit>> {
        Ok(None)
    }
    let _l = packager::hooks::set_lookup(packager::lookup::Client { extra: hit, aur: n });
    let deb = fixture_deb();
    let code = run([
        "install".into(),
        "-y".into(),
        deb.to_string_lossy().into(),
        "--workdir".into(),
        std::env::temp_dir()
            .join("packager-sysn")
            .to_string_lossy()
            .into(),
    ]);
    assert_eq!(code, 1);
    assert!(!Command::new("pacman")
        .args(["-Q", "packager-systest"])
        .status()
        .unwrap()
        .success());
}
