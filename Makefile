.PHONY: test test-system build
build:
	cargo build --release
test:
	RUST_TEST_THREADS=1 cargo test --lib
test-system:
	RUST_TEST_THREADS=1 cargo test --features system --test system -- --nocapture --include-ignored
