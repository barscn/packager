# Packager design

**Date:** 2026-08-12
**Status:** Draft for review
**Working name:** `packager` (rename later if needed)

An Arch Linux utility that turns a local `.deb` or `.rpm` into a real pacman package, as a **temporary bridge** until extra or an AUR maintainer ships a native package. It is not a package manager, not a replacement for the AUR, and not a bidirectional converter.

Default path: **preview → write a PKGBUILD → `makepkg` → `pacman -U`**. Packager-facing mode stops at the PKGBUILD (`scaffold`).

## 1. Purpose

Vendors often ship only Debian or RPM packages (printer drivers, VPN clients, bank software, other closed binaries). Arch users already have extra, the AUR, Flatpak, AppImage, and Distrobox. This utility exists for the leftover case: *there is no native package yet, I have a vendor file, I want a pacman-tracked install I can undo.*

When extra or the AUR later has the software, `packager status` tells the user to switch. The utility is meant to get out of the way.

### 1.1 Users

- **Desktop Arch user (default):** `packager foo.deb` previews, converts, and installs.
- **Someone who will maintain a package:** `packager scaffold foo.deb` writes a PKGBUILD they can edit and publish.

### 1.2 Non-goals (v1)

- Arch → `.deb` / `.rpm`
- Replacing extra, the AUR, `yay` / `paru`, Flatpak, or Distrobox
- A search interface or third-party repo of converted packages
- GUI, daemon, pacman hook, or systemd timer
- Running or rewriting vendor maintainer scripts by default
- Rewriting Debian multiarch or Fedora `lib64` paths into Arch paths
- Auto-installing missing dependencies from extra or the AUR (`makepkg -s` is not used)
- Signature verification of the vendor file
- Repairing a previous failed convert

## 2. Command surface

No daemon. Root only for `pacman -U` (install) and an optional `pacman -R` from `forget`. `convert` and `scaffold` stay unprivileged.

```text
packager foo.deb              # default = install
packager foo.rpm              # same path
packager install foo.deb      # explicit

packager convert foo.deb      # preview + makepkg, do not install
packager scaffold foo.deb     # write PKGBUILD (and .install only if allowed), stop

packager status               # converted packages we still track; native extra/AUR now?
packager forget <pkg>         # stop tracking; ask whether to pacman -R as well
```

### 2.1 Default `install` flow

1. Identify `.deb` / `.rpm` and read metadata.
2. Look up extra and the AUR by name (and a static alias table in the repo — a handful of well-known Debian/RPM names such as `google-chrome-stable` → `google-chrome`. Not scraped. Grows by patch only).
3. Print the preview.
4. Stop if a native package exists, unless `--force`.
5. Confirm (`Enter` to proceed). `--yes` skips the prompt after the preview is printed.
6. Write a PKGBUILD in a work dir → `makepkg` → `pacman -U` → record state.

### 2.2 Flags

| Flag | Effect |
|---|---|
| `--force` | Continue even if extra/AUR already has a match, or if packaged files would overwrite another package’s files |
| `--allow-scripts` | Copy vendor scripts into a pacman `.install` (pacman runs them on `-U`; we never run them during convert) |
| `--yes` / `-y` | Skip the confirm prompt after preview. Also allows `pacman --noconfirm` on the action the user asked for (`install`, or `forget` after they already said yes to remove) |
| `--workdir DIR` | Where to put the PKGBUILD and built package (default: cache under the user’s home) |

`scaffold` is the same pipeline stopped after the PKGBUILD is written.

### 2.3 Out of the CLI in v1

Search-as-a-package-manager, GUI, reverse conversion, enabling systemd units, background watchers. `status` is on-demand only.

## 3. Preview

The preview always prints. Convert/install continue only after it (and confirm, unless `-y`).

Order of report:

1. **Identity** — source file, format, proposed `pkgname` / `pkgver` / `arch`.
2. **Native match** — extra and AUR hits for that name and a few obvious aliases (example: `google-chrome-stable` → `google-chrome`). Match → **hard stop** unless `--force`.
3. **Dependencies** — mapped to extra/multilib; mapped to AUR (called out, not auto-installed); unmapped.
4. **Scripts** — if `postinst` / `%post` / equivalents exist: count and a short summary of commands they would call. Default: not turned into `.install`. With `--allow-scripts`: “will become `.install`; pacman will run them on `-U`.”
5. **Layout / ABI** — Debian multiarch paths (`/usr/lib/x86_64-linux-gnu`), Fedora `/usr/lib64`, 32-bit-on-64, interpreter/glibc notes from `readelf` when available. Warnings, not stops.
6. **Conflicts** — packaged files that would overwrite a path already owned by another pacman package. **Hard stop** unless `--force`.
7. **Verdict** — proceed / proceed with warnings / blocked.

### 3.1 Stops vs warnings

| Situation | Default |
|---|---|
| extra/AUR already has it | stop (`--force` to continue) |
| file conflict with an installed package | stop (`--force` to continue) |
| unmapped dependencies | warn, continue |
| vendor scripts present | warn, skip scripts (unless `--allow-scripts`) |
| odd lib paths / possible ABI mismatch | warn, continue |
| not linux / wrong arch | stop |
| extra/AUR lookup failed (offline) | cannot claim “no native package”; say lookup failed and **stop** unless `--force` |

If blocked: write nothing, exit non-zero.

Unmapped depends are listed in the preview and **omitted** from the PKGBUILD `depends` array. A fake `libfoo-dev` would make `pacman -U` fail for no good reason.

## 4. State and remind-later

Recorded only after a successful `install` (something landed in the pacman database).

Per package, JSON at:

`~/.local/share/packager/installed/<pkgname>.json`

Fields: `pkgname`, `pkgver`, `arch`, source file name, source checksum, format (`deb` / `rpm`), install time, whether `--allow-scripts` / `--force` were used, workdir path if it still exists.

The generated PKGBUILD sets:

```text
packager = "packager <packager@local>"
```

so `pacman -Qi` still identifies these installs if the JSON is gone.

If the user ran `sudo packager install …`, write state as the original user (`SUDO_USER` / `logname`), not as root. No `/var/lib` store in v1.

`convert` and `scaffold` do not write this state.

### 4.1 `packager status`

For each recorded package:

1. Still installed? If `pacman -Q` says no → mark stale, suggest `forget`.
2. Query extra, then AUR, same lookup as preview.
3. Print one line per package: converted version, native hit if any, recommendation.

```text
zoom  6.2.0-1  (from zoom_amd64.deb)
  extra has zoom 6.2.3-1  →  replace: sudo pacman -S zoom
acme-vpn  1.4-1  (from acme-vpn.rpm)
  no native package
```

`status` does not uninstall, replace, or notify on `pacman -Syu`. No hook or timer in v1.

If extra/AUR lookup fails, still list local records and say lookup failed. Do not print “no native package.”

Replacement is a documented two-step: install the native package, then `packager forget <pkg>` if the name differs. If the name is the same, `pacman -S` already replaces our package; `status` then treats the record as stale.

### 4.2 `packager forget <pkg>`

Always asks whether to run `pacman -R <pkg>` as well. Default answer is **no**.

| User choice | Success | `pacman -R` fails |
|---|---|---|
| No | delete JSON only; package stays installed | n/a |
| Yes | run `pacman -R <pkg>` (not `-Rns`); then delete JSON | JSON **not** deleted; still tracking the installed package |

`forget` never uninstalls unless the user answers yes to that prompt.

## 5. Conversion pipeline

Approach: the PKGBUILD is the conversion artifact. We do not wrap debtap, alien, or fpm.

Default workdir (unless `--workdir`):

`~/.cache/packager/<pkgname>-<pkgver>/`

Contains: extracted payload, generated `PKGBUILD`, optional `.install`, `scripts.orig/` (vendor scripts, always saved for reading), then the built `.pkg.tar.zst`.

### 5.1 Steps

1. **Unpack metadata only** — Debian `control` + maintainer scripts, or RPM header + scriptlets. Enough for preview. If preview blocks, stop here.
2. **Unpack files** with `bsdtar` (same extractor `makepkg` uses).
3. **Map dependencies** from the union of:
   - declared Depends / Requires
   - `readelf` `NEEDED` entries looked up via `pkgfile`
   - a known Debian/RPM → Arch name table for common packages
4. **Write PKGBUILD** in the usual `*-bin` style. Name and version rules:
   - `pkgname`: vendor package name, lowercased, `_` → `-`
   - `pkgver`: upstream version with Arch-illegal characters (`:`, `/`) replaced by `.` or `_` per `makepkg` rules
   - `pkgrel`: always `1` for a converted package
   - `epoch`: Debian/RPM epoch if present, otherwise omitted
   - `arch`: `amd64`/`x86_64` → `x86_64`; `arm64`/`aarch64` → `aarch64`; anything else → stop (wrong arch)
   - `depends`: mapped names only
   - `conflicts` / `provides` / `replaces`: only names we can map the same way as depends; we do not invent extra `provides`
   - `packager` field as above
   - `package()` installs the extracted tree into `$pkgdir`
5. **Layout:** files stay where the vendor put them. No rewrite of `/usr/lib/x86_64-linux-gnu` or `/usr/lib64` in v1 (rewrites break `RPATH` more often than they help). Preview already warned.
6. **`.install`:** only if `--allow-scripts`. Scripts are copied as faithfully as possible. We do **not** rewrite `update-alternatives`, `update-rc.d`, `debconf`, or RPM scriptlet helpers into Arch. Without the flag, scripts stay in `scripts.orig/` and are not packaged.
7. **`makepkg`** without `-s`. Missing runtime depends are pacman’s problem at `-U` time.
8. **Finish:**
   - `install` — `pacman -U`, then write JSON state
   - `convert` — stop after the `.pkg.tar.zst` exists
   - `scaffold` — stop after `PKGBUILD` (and `.install` if allowed)

No network except extra/AUR lookups for preview and `status`. The `.deb`/`.rpm` is a local file. Vendor scripts are never executed during convert. Only pacman would run `.install` on `-U`, and only if the user passed `--allow-scripts`.

### 5.2 External tools (must exist)

- `bsdtar` — unpack `.deb` / `.rpm` payload
- `readelf` (binutils) — `NEEDED`, interpreter, class
- `pkgfile` — map shared libraries to Arch packages
- `makepkg` / `fakeroot` — build the pacman package
- `pacman` — `-U`, `-Q`, `-R`, file ownership, extra sync DB
- extra/AUR name lookup — local pacman sync DB for extra; AUR RPC for AUR

## 6. Error handling

- Fail before writing a package whenever the preview would have blocked.
- Never leave a half-installed pacman package. If `pacman -U` fails, do not write state and do not roll back other packages.
- Keep the workdir on failure so it can be inspected. Success may keep it too. Do not auto-delete in v1. `--workdir` is never deleted by us.
- Non-zero exit on any stop or hard failure. Warnings-only still exit 0 if the user proceeded.

| Stage | Failure | Behavior |
|---|---|---|
| Not a `.deb`/`.rpm`, unreadable, wrong arch | stop | nothing written |
| Preview hard-stop (native match, file conflict, lookup failed) | stop | nothing written |
| Metadata/files unpack | stop | partial workdir kept |
| `makepkg` fails | stop | no `pacman -U`, no state |
| `pacman -U` fails (deps, conflict, user abort) | stop | no JSON state; built archive stays in workdir |
| `status` lookup fails | warn | still list local records; do not claim “no native package” |
| `forget` + yes to remove, `pacman -R` fails | error | JSON not deleted |
| `forget`, no to remove | success | JSON deleted only |

`--allow-scripts` and a `.install` that is not valid shell is a normal `makepkg` / `pacman -U` failure, not a special recovery path.

We stream pacman output. We do not parse-and-hide it. We never pass `--noconfirm` unless the user passed `--yes` and the action is the one they asked for.

v1 does not: repair a broken previous run (use a new run or a fresh `--workdir`); verify vendor signatures (preview may say “unverified source”); rewrite paths after a failed install.

## 7. Components

Implementation language is **Rust** (edition 2021). The design is a CLI that shells out to the tools in §5.2.

Logical units (each one purpose, testable without the others):

| Unit | Does | Depends on |
|---|---|---|
| `cli` | Parse argv, dispatch subcommands, prompts | everything else only via their public functions |
| `source` | Detect format; parse Debian control / RPM header; list files; extract with `bsdtar` | `bsdtar` |
| `elf` | `readelf` NEEDED / class / interpreter | `readelf` |
| `depmap` | Name table + `pkgfile` lookup; produce mapped / AUR / unmapped buckets | `elf`, `pkgfile` |
| `lookup` | extra via local sync DB; AUR via RPC; alias list | `pacman`, network for AUR only |
| `preview` | Build the report and verdict (stop / warn / proceed) | `source`, `depmap`, `lookup`, `elf` |
| `pkgbuild` | Render PKGBUILD and optional `.install` | `source`, `depmap` |
| `build` | Run `makepkg` in the workdir | `pkgbuild` |
| `install` | `pacman -U` | `build` |
| `state` | JSON records; original-user path when invoked via sudo | filesystem |
| `status` / `forget` | Remind-later; optional `pacman -R` | `state`, `lookup`, `pacman` |

A unit’s callers should not need to know how it talks to `pacman` or `bsdtar`.

## 8. Testing

Converters can damage a system. Default tests must not require root or `pacman -U`.

### 8.1 Pure unit tests (no root, no pacman)

- Metadata parse fixtures (tiny canned `.deb` / `.rpm` generated in the harness, not live vendor files)
- Debian/RPM → Arch name table
- Preview verdict: native match, file conflict, wrong arch, scripts present, lookup failure → stop vs warn
- Layout/ABI **detection**: fixtures with `/usr/lib/x86_64-linux-gnu`, `/usr/lib64`, 32-bit ELF on x86_64, missing interpreter → warnings printed, files **not** rewritten
- PKGBUILD renderer: expected `pkgname`, `depends`, `packager` field; no `.install` unless allow-scripts
- State JSON read/write; `forget` without remove

### 8.2 Pipeline tests (user namespace, `makepkg`, no `pacman -U`)

- Minimal fake `.deb` and `.rpm` (one binary, one dep, one `postinst`)
- `scaffold` writes a PKGBUILD `makepkg` accepts
- `convert` produces a `.pkg.tar.zst` whose file list matches the payload
- Without `--allow-scripts`, archive has no `.INSTALL`; with it, it does
- Unmapped depends omitted from the PKGBUILD
- Extra lookup hits the **real local pacman sync DB**
- AUR lookup uses **recorded real RPC fixtures** in CI

### 8.3 `@system` tests (opt-in; not in the default test command)

- `install` a fixture package; `pacman -Q` sees it; `status` lists it
- `forget` without remove: package still installed, JSON gone
- `forget` with remove: package gone, JSON gone
- Native-match stop with a mocked extra/AUR hit

### 8.4 Network

One optional live AUR RPC call, **off by default**. Not required to pass CI.

### 8.5 Fixtures

No Zoom/Chrome (or other vendor) blobs in the repo. Generate tiny packages in the harness.

### 8.6 Not tested in v1

- Every Debian/RPM maintainer-script dialect (we do not rewrite scripts)
- Path **rewrite** (the feature does not exist)

Path **detection** is required (§8.1).

### 8.7 Bar for “v1 is usable”

- Unit + pipeline tests pass on every commit
- A human ran `@system` once against a real `.deb` and a real `.rpm` (names written down; files not committed)

## 9. Key decisions

| Decision | Choice | Why |
|---|---|---|
| Role | Temporary utility, not a product or package manager | Helps until extra/AUR exists; then get out of the way |
| Direction | `.deb` and `.rpm` → pacman only | Reverse is a packager problem (fpm) and ABI-unsafe from rolling Arch |
| Default verb | Preview → convert → install | User-friendly; skip install with the `convert` subcommand |
| Conversion method | Generate PKGBUILD, then `makepkg` | Arch-native; `scaffold` is free; pacman owns files |
| Formats in v1 | Both `.deb` and `.rpm` | debtap is deb-only; RPM is the gap |
| Native package exists | Hard stop unless `--force`; `status` reminds later | Matches “until official exists” |
| Vendor scripts | Off unless `--allow-scripts`; never run during convert | Foreign `postinst`/`%post` is the main way conversions break machines |
| Lib paths | Detect and warn; do not rewrite | Rewrites break `RPATH` more often than they help |
| Unmapped depends | Omit from PKGBUILD | Fake names make install fail |
| `makepkg -s` | Do not use | We are not an AUR helper |
| `forget` | Always ask about `pacman -R`; default no; keep JSON if remove fails | Tracking and uninstall are separate |
| State location | Per-user JSON; `packager` field in PKGBUILD | Works with sudo via `SUDO_USER`; no v1 system daemon |
| Implementation language | Rust (edition 2021) | Single binary, no runtime; design is still subprocess + files |

## 10. Open questions

None that block this spec. The binary name stays `packager` unless we rename later.

## 11. What v1 looks like when it is done

```text
$ packager vendor-vpn_1.4_amd64.deb
source: vendor-vpn_1.4_amd64.deb  (deb)
pkgname: vendor-vpn  1.4-1  x86_64
native: no extra/AUR match
depends: openssl, systemd
unmapped: (none)
scripts: 1 postinst (update-desktop-database) — will not be packaged
layout: /usr/lib/x86_64-linux-gnu/libvendor.so  (warning: Debian multiarch path)
conflicts: (none)
verdict: proceed with warnings

Install vendor-vpn 1.4-1? [Y/n]
```

After extra later ships `vendor-vpn`:

```text
$ packager status
vendor-vpn  1.4-1  (from vendor-vpn_1.4_amd64.deb)
  extra has vendor-vpn 1.4.2-1  →  replace: sudo pacman -S vendor-vpn
```
