.PHONY: bootstrap dev build bundle release acceptance test test-unit test-integration lint fmt

TAURI_DIR := apps/desktop/src-tauri

bootstrap:          ## Download Tailwind CLI and vendored HTMX
	./scripts/bootstrap.sh

dev:                ## Run the desktop app in debug mode
	cd $(TAURI_DIR) && cargo tauri dev

build:
	cargo build --workspace

TARGET := aarch64-apple-darwin
VERSION := $(shell sed -n 's/^  "version": "\(.*\)",/\1/p' $(TAURI_DIR)/tauri.conf.json)
BUNDLE_DIR := target/$(TARGET)/release/bundle
APP := $(BUNDLE_DIR)/macos/NLMX.app

# Ad-hoc signatures have no Team ID, and the hardened runtime's library validation then refuses
# every bundled dylib: the hardened runtime is enabled only with a real signing identity.
bundle:             ## dist/NLMX.dmg (Apple Silicon, ad-hoc signed unless APPLE_SIGNING_IDENTITY is set) + verification
	./scripts/stage-runtime.sh
	cd $(TAURI_DIR) && cargo tauri build --target $(TARGET) --bundles app,dmg $(if $(APPLE_SIGNING_IDENTITY),,--config '{"bundle":{"macOS":{"hardenedRuntime":false}}}')
	mkdir -p dist
	cp "$(BUNDLE_DIR)/dmg/NLMX_$(VERSION)_aarch64.dmg" dist/NLMX.dmg
	cp dist/NLMX.dmg dist/NLMX-$(VERSION).dmg
	./scripts/verify-bundle.sh "$(APP)" dist/NLMX.dmg

release:            ## Signed + notarized DMG (needs APPLE_SIGNING_IDENTITY and APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID or an API key)
	@test -n "$$APPLE_SIGNING_IDENTITY" || { echo "defina APPLE_SIGNING_IDENTITY (Developer ID Application: …)"; exit 1; }
	@test -n "$$APPLE_ID$$APPLE_API_KEY" || { echo "defina APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID ou APPLE_API_ISSUER/APPLE_API_KEY/APPLE_API_KEY_PATH para notarizar"; exit 1; }
	$(MAKE) bundle

acceptance:         ## Installs dist/NLMX.dmg in a temp folder and verifies it end to end (downloads the model, ~640 MB)
	./scripts/acceptance.sh

test:               ## Everything that needs no real model (default)
	cargo test --workspace

test-unit:          ## Unit tests (pure logic, inside each crate)
	cargo test --workspace --lib

test-integration:   ## Crate integration tests (SQLite, PDFium, process fakes) + doc tests
	cargo test --workspace --tests --exclude nlmx-workspace-tests
	cargo test --workspace --doc

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all
