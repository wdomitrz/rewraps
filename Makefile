# Everything is built here and in CI; nothing generated is committed.

.PHONY: install fmt fmt-check lint test wasm bundle vsix check clean

install:
	npm ci

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

# The engine, as the extension consumes it: a wasm module plus the small JS
# shim wasm-bindgen generates. Both land in dist/ and are gitignored.
wasm:
	mkdir -p dist
	cargo build --release -p rewrap-wasm --target wasm32-unknown-unknown
	wasm-bindgen --target web --out-dir src/pkg \
		target/wasm32-unknown-unknown/release/rewrap_wasm.wasm
	cp src/pkg/rewrap_wasm_bg.wasm dist/rewrap_wasm_bg.wasm

bundle: wasm
	npm run compile

check: fmt-check lint test bundle
	npm test

vsix: clean bundle
	npx vsce package --no-dependencies

clean:
	rm -rf dist out *.vsix
