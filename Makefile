CARGO ?= cargo

.PHONY: test package source run

test:
	$(CARGO) test --locked --workspace --manifest-path rust/Cargo.toml

package:
	$(CARGO) run --locked --manifest-path rust/Cargo.toml -p xtask -- package

source:
	$(CARGO) run --locked --manifest-path rust/Cargo.toml -p xtask -- source

run:
	$(CARGO) run --manifest-path rust/Cargo.toml -- --private private --cache cache --local-voice
