# v1 human smoke

| File | Distro origin | Date | Result |
|---|---|---|---|
| `~/Downloads/chatgpt_amd64.deb` (not committed; OpenAI Linux desktop, 334M, control/data xz) | Debian/Ubuntu vendor package | 2026-08-13 | `packager convert -y` produced `chatgpt-26.803.81509-1-x86_64.pkg.tar.zst`. First run deadlocked on `bsdtar` pipes; fixed in `f6247e2`. Preview: no extra/AUR match; scripts not packaged. |
| (real .rpm, not committed) | | | not run yet |
