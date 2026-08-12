# Agent notes

For coding agents. Humans: `README.md`.

Policy source: `docs/superpowers/specs/2026-08-12-packager-design.md`.
Implementation history: `docs/superpowers/plans/2026-08-12-packager.md`.
Host smoke (no vendor blobs): `docs/superpowers/plans/v1-system-smoke.md`.

Do not fork policy into this file. If a change moves a default, stop, or CLI contract, update the spec in the same change. Update `README.md` only when user-facing commands/flags change.

## Repo

Rust 2021, rustc 1.75+. Binary `packager` + lib `packager`. No async, no `clap` (manual argv in `cli.rs`), no tokio.

`src/main.rs` is `std::process::exit(packager::cli::run(args))`. All behavior lives in the lib.

Local `.deb`/`.rpm` → preview → PKGBUILD → `makepkg -f` → optional `pacman -U`. Temporary bridge. Not a package manager, not an AUR helper, not Arch→deb/rpm.

## Hard stops (do not implement)

- Reverse conversion; search-as-PM; GUI/daemon/hook/timer
- `debtap` / `alien` / `fpm` wrappers
- Executing vendor `postinst` / `%post` during convert
- Rewriting Debian/RPM scripts (`update-alternatives`, `update-rc.d`, `debconf`, RPM helpers) into Arch
- Rewriting `/usr/lib/x86_64-linux-gnu` or `/usr/lib64` (detect + warn only)
- `makepkg -s` (sync deps). `build.rs` must keep rejecting `-s`
- Inventing unmapped names into PKGBUILD `depends`
- Claiming “no native package” when extra/AUR lookup failed
- Writing state JSON unless `pacman -U` succeeded
- Deleting workdirs (including `--workdir`) on success or failure
- `chown -R` on a user `--workdir` (only files this run created)
- `makepkg` as root. If euid 0, `runuser -u $SUDO_USER`. Root + empty `SUDO_USER` is an error
- `--noconfirm` unless the user passed `--yes` **and** the action is the one they asked for
- Vendor `.deb`/`.rpm` blobs in git — generate via `testpkg`
- A second error type. Use `crate::error::{Error, Result}` (`Error::msg`)
- Growing `lookup::alias` by scrape. Patch-only table (today: `google-chrome-stable` → `google-chrome`)

## Invariants

- Preview hard-stop is **before** extract. Blocked ⇒ no `payload/`, no `PKGBUILD`, exit 1
- `--force` overrides: native extra/AUR hit, lookup failure, file conflict. **Not** wrong arch
- `pkgname`: trim, lowercase, `_` → `-`. `pkgver`: `:` → `.`, `/` → `_`. `pkgrel` always `1`
- Arch: `amd64`/`x86_64` → `x86_64`; `arm64`/`aarch64` → `aarch64`; else block
- PKGBUILD: `packager = "packager <packager@local>"` (`ident::PACKAGER_FIELD`) and `export PACKAGER=...`
- File conflicts: `pacman -Qo` on **absolute** path (`Path::new("/").join(rel)`). Skip listed directories
- State path: `~/.local/share/packager/installed/<pkgname>.json`. Under sudo: `SUDO_USER`, then `logname` if `SUDO_USER` is set-but-empty. Unset `SUDO_USER` is a normal user run — do not consult `logname`
- `convert` / `scaffold` never write state
- `forget` always asks about `pacman -R` (default no). `--yes` does **not** skip that prompt. If `-R` fails, keep the JSON
- `status` AUR hit: do not print `pacman -S` (AUR is not extra)
- Default workdir: `~/.cache/packager/<pkgname>-<pkgver>/`
- Pacman stdout/stderr streamed, not parsed-and-hidden
- Exit: usage/parse `2`; blocked / declined / pipeline fail `1`; success (incl. proceed-with-warnings after confirm) `0`

## Pipeline order (`cli::pipeline`)

1. `source::parse_meta` + `source::list_payload`
2. `lookup::candidates` + hooked client `find`; declared `depmap::map_names`
3. `preview::evaluate` (ELF vector is empty here) → print → `Verdict::Blocked`? return 1
4. Confirm unless `-y`
5. `source::extract` into `workdir/payload`
6. `convert`/`install` only: `hooks::scan` NEEDED union + extra layout lines
7. `pkgbuild::write` (always `scripts.orig/` if scripts exist; `.install` only with `--allow-scripts`)
8. `convert`/`install`: `build::makepkg`
9. `install`: `hooks::upgrade` then `state::write`. Failed `-U` ⇒ no JSON

`scaffold` stops after step 7 (no NEEDED union, no makepkg).

## Module map

| Path | Owns |
|---|---|
| `src/cli.rs` | argv, `run`, install/convert/scaffold pipeline |
| `src/source.rs` + `source/{deb,rpm,extract}.rs` | detect, control/header, `list_payload`, `bsdtar` extract |
| `src/ident.rs` | `Format`, `Arch`, name/ver/arch, `PKGREL`, `PACKAGER_FIELD` |
| `src/preview.rs` | `Input` / `Report` / `Verdict` / `format_report` |
| `src/lookup.rs` | extra `pacman -Si`, AUR RPC (`ureq`), `candidates` / `alias` |
| `src/depmap.rs` | constraint strip, static table, `Resolver` / `PkgfileResolver`, buckets |
| `src/elfinfo.rs` | `readelf` NEEDED / class / interpreter, layout warnings |
| `src/pkgbuild.rs` | PKGBUILD + optional `{pkgname}.install` |
| `src/build.rs` | `makepkg -f` only; SUDO_USER drop |
| `src/pm.rs` | `pacman -U/-R/-Q/-Qo` |
| `src/state.rs` | JSON records; `set_data_dir_for_test` guard |
| `src/status.rs` | `status` text |
| `src/hooks.rs` | process-global inject + restore-on-drop `Guard` |
| `src/testpkg.rs` | fixture `.deb` (pure) / `.rpm` (`rpmbuild`) |
| `src/error.rs` | only `Error` |
| `tests/system.rs` | opt-in `@system` (`--features system`); panics if not root |

External tools on `PATH`: `bsdtar`, `readelf`, `pkgfile`, `makepkg`/`fakeroot`, `pacman`. RPM fixtures also need `rpmbuild`.

## Commands

```bash
make build                         # cargo build --release
make release                       # cargo build --release --locked (GitHub tag artifacts)
make test                          # RUST_TEST_THREADS=1 cargo test --lib
sudo -E make test-system           # root + SUDO_USER required
cargo fmt --all -- --check
```

`make test` does **not** compile or run `tests/system.rs`. Plain `sudo make test-system` (no `-E`) can drop `SUDO_USER` and fail `makepkg` drop.

Never claim tests passed without running the matching target. Default gate is `make test`. Do not run `test-system` unless the change needs a real `pacman -U`/`-R` and the host allows it.

## Tests (read before adding one)

- **`RUST_TEST_THREADS=1` is mandatory.** Hooks, `cli::HANDLE`, and `state::TEST_DATA_DIR` are process-global.
- Every override is a drop guard (`hooks::set_*`, `state::set_data_dir_for_test`). Hold it for the whole body, including panic paths. Do not set a hook without a guard.
- Lib tests: inject lookup/confirm/scan/upgrade/remove/query/forget_remove/owned_by/resolver. No `pacman -U`. No root.
- Drive the CLI with `cli::run(["scaffold"|…])` + temp `--workdir` + isolated data dir.
- Fixtures: `testpkg::{write_deb, DebSpec, write_rpm, RpmSpec}`. Prefer `.deb` unless the change is RPM-specific (`write_rpm` needs `rpmbuild`).
- AUR RPC fixture: `src/lookup/testdata/aur_info.json`. Extra may hit the real local sync DB (`pacman -Si`).
- Some `#[ignore]` lib tests need `makepkg` on `PATH`; do not `return` early to skip — they are ignored on purpose.
- `tests/system.rs`: must panic when not root so CI cannot silently skip. Stub lookup (`hooks::set_lookup`) so a real extra/AUR hit does not block.

## Style

- rustfmt; edition 2021; no new deps without a concrete need already used in-tree
- Module-level `//!` for the contract; inline comments only for non-obvious constraints
- Keep units narrow: callers should not know how a module shells out to `pacman`/`bsdtar`/`readelf`
- Prefer extending an existing hook over adding another global
