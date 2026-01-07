CRATE := templar-vault-client
LIB_NAME := templar_vault_client
DIST_DIR := dist
PY_OUT := $(DIST_DIR)/python

MODE ?= debug

UNIFFI_BINDGEN := cargo run -p $(CRATE) --bin uniffi-bindgen --

ifeq ($(MODE),release)
	CARGO_BUILD_FLAGS := --release
	PROFILE := release
else
	CARGO_BUILD_FLAGS :=
	PROFILE := debug
endif

ifeq ($(OS),Windows_NT)
	DYLIB := ../../target/$(PROFILE)/$(LIB_NAME).dll
else
	UNAME_S := $(shell uname -s)
	ifeq ($(UNAME_S),Darwin)
		DYLIB := ../../target/$(PROFILE)/lib$(LIB_NAME).dylib
	else
		DYLIB := ../../target/$(PROFILE)/lib$(LIB_NAME).so
	endif
endif

.PHONY: gen python smoke-test

gen:
	cargo run --features=uniffi/cli --bin uniffi-bindgen $@

python: $(PY_OUT)
	cargo build -p $(CRATE) $(CARGO_BUILD_FLAGS)
	$(UNIFFI_BINDGEN) generate --language python --library "$(DYLIB)" --out-dir "$(PY_OUT)" --no-format
	cp "$(DYLIB)" "$(PY_OUT)/"

smoke-test: python
	PYTHONPATH="$(PY_OUT)" python3 smoke_test.py

$(PY_OUT):
	mkdir -p "$(PY_OUT)"
