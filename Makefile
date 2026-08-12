.PHONY: test test-system build release
build:
	cargo build --release
release:
	cargo build --release --locked
test:
	RUST_TEST_THREADS=1 cargo test --lib
test-system:
	RUST_TEST_THREADS=1 cargo test --features system --test system -- --nocapture --include-ignored
