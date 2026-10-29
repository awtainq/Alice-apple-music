PYTHON ?= python3
CARGO ?= cargo

.PHONY: test test-legacy package run

test:
	$(CARGO) test --locked --manifest-path rust/Cargo.toml
	$(PYTHON) -m unittest discover -s rust/tests -v

test-legacy:
	$(PYTHON) -m unittest discover -s tests -v

package:
	$(PYTHON) rust/package.py

run:
	$(CARGO) run --manifest-path rust/Cargo.toml -- --private private --cache cache --local-voice
