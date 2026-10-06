# Distribution (macOS, Apple Silicon)

## Building the DMG

```sh
make bootstrap   # once: Tailwind, HTMX, PDFium, llama.cpp (+ runtime staging)
make bundle      # dist/NLMX.dmg (and dist/NLMX-<version>.dmg), with verification
```

`make bundle`:

1. `scripts/stage-runtime.sh` copies the runtime from `runtime/` into the bundler layout, in git-ignored folders:
   - `src-tauri/binaries/llama-server-aarch64-apple-darwin`: the sidecar, with rpath `@executable_path/../Frameworks`;
   - `src-tauri/frameworks/*.dylib`: libllama, libggml*, libmtmd and libpdfium;
   - `src-tauri/licenses/`: PDFium, llama.cpp, sqlite-vec and SQLite.
2. `cargo tauri build --target aarch64-apple-darwin --bundles app,dmg`.
3. Copies the DMG to `dist/`.
4. `scripts/verify-bundle.sh` checks:
   - no `.gguf` and no file above 50 MB;
   - every binary is `arm64` only;
   - dependencies only from the bundle or the system;
   - valid signature;
   - `--self-check` with an empty environment;
   - DMG contents.

## App contents

```
NLMX.app/Contents/
  MacOS/nlmx-desktop      Tauri/Rust app (SQLite and sqlite-vec compiled in)
  MacOS/llama-server        embeddings sidecar (llama.cpp b11349)
  Frameworks/               libpdfium (chromium/7881) + llama.cpp/ggml libraries (Metal)
  Resources/licenses/       licenses of the bundled libraries
```

- **GGUF models are not in the bundle.** They are downloaded by the Model Manager (Modelos), always with confirmation, to `~/Library/Application Support/dev.nlmx.desktop/models/`.
- **Apple Foundation Models belongs to macOS** (`/usr/bin/fm`). The app requires macOS 27 and Apple Intelligence; the license is accepted once with `sudo fm license`.
- **Apple Silicon only:** `LSRequiresNativeExecution` and `LSArchitecturePriority = arm64`. The app also refuses Rosetta at runtime.

## Signing and notarization

Without variables, the app comes out **ad-hoc signed** (`signingIdentity: "-"`) and **without hardened runtime**: an ad-hoc signature has no Team ID, and the hardened runtime's library validation would reject every dylib in `Frameworks`. `verify-bundle.sh` caught this on the first build. It works on this Mac; on others, Gatekeeper asks for right-click › Open the first time.

To distribute for real:

```sh
export APPLE_SIGNING_IDENTITY="Developer ID Application: Name (TEAMID)"
# notarization — Apple ID with an app-specific password…
export APPLE_ID="you@example.com" APPLE_PASSWORD="xxxx-xxxx-xxxx-xxxx" APPLE_TEAM_ID="TEAMID"
# …or an App Store Connect API key
# export APPLE_API_ISSUER=… APPLE_API_KEY=… APPLE_API_KEY_PATH=…/AuthKey_XXXX.p8
make release
```

Tauri signs the app, the sidecar and every library in `Frameworks` with hardened runtime (empty `Entitlements.plist`, no exceptions and no sandbox). It then submits for notarization and staples the ticket. `verify-bundle.sh` then also requires `spctl --assess` and `stapler validate`.

## Where the app keeps things

| What | Path |
|---|---|
| database, PDF library, indexes | `~/Library/Application Support/dev.nlmx.desktop/` (`nlmx.sqlite3`, `library/`) |
| downloaded models | `…/models/<id>/<version>/` |
| app JSON logs, llama-server log | `…/logs/` (Configurações › Mostrar logs no Finder) |
| pidfiles, llama-server key, `fm serve` log | `…/run/` |
| `fm serve` socket | `$TMPDIR/nlmx-fm-<pid>-<n>.sock` |

Development variables (`NLMX_PDFIUM_PATH`, `NLMX_LLAMA_SERVER`, `NLMX_EMBEDDING_CONFIG`) still work as overrides. The fallback paths to `runtime/` and `NLMX_START_PATH` only exist in debug builds.

## Updates

There is no auto-updater and no network check: the app's only network use remains the user-requested model download. To update, install the new DMG over the old one (drag to Applications and replace):

- **Data kept:** documents, conversations, indexes and models live in Application Support and are kept;
- **Database upgraded automatically:** migrations run on launch. They are numbered and reversible, and new versions only add;
- **Embedding server replaced:** a `llama-server` left behind by a previous version is replaced (the pidfile compares the binary).
- **Coming from PDF RAG 0.1.0** (old name and bundle id, `dev.pdfrag.desktop`): on first launch the data folder is moved to `dev.nlmx.desktop` (`src-tauri/src/legacy_data.rs`), with the database, logs and stored paths updated. An old copy of PDF RAG opened afterwards no longer sees that data.

Configurações › Sobre (Settings › About) shows the version. With `NLMX_DOWNLOAD_URL=https://…` set at build time, the **Página de downloads** (Downloads page) button also appears.

## Publishing from GitHub

Pushing a `vX.Y.Z` tag builds and publishes the DMG through `.github/workflows/release.yml` — unsigned and not notarized for now. See [CI.md](CI.md).

## Before publishing

- [ ] `make lint && make test`
- [ ] Version updated in `apps/desktop/src-tauri/tauri.conf.json` and the workspace `Cargo.toml`
- [ ] `make release` (or `make bundle` for tests) with no verification failures
- [ ] `make acceptance`: clean install, first launch, model download + checksum, offline use (RAG, chat, citations, viewer) with the installed app's runtime
- [ ] Install the DMG on a clean Mac, without the repository: import a PDF, download the model in Modelos, ask in the Chat
