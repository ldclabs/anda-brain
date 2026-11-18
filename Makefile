.PHONY: lint fix test

lint:
	@cargo fmt --check
	@cargo clippy --locked --all-targets --all-features -- -D warnings

fix:
	@cargo fmt --all
	@cargo clippy --fix --workspace --tests

test:
	@RUST_MIN_STACK=16777216 cargo test --workspace --all-features -- --nocapture
