PREFIX ?= /usr/local
DESTDIR ?=

.PHONY: test test-system build release install uninstall
build:
	cargo build --release
release:
	cargo build --release --locked
test:
	RUST_TEST_THREADS=1 cargo test --lib
test-system:
	RUST_TEST_THREADS=1 cargo test --features system --test system -- --nocapture --include-ignored
install:
	install -Dm755 target/release/packager "$(DESTDIR)$(PREFIX)/bin/packager"
uninstall:
	rm -f "$(DESTDIR)$(PREFIX)/bin/packager"
