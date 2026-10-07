#!/usr/bin/env bash
# Downloads build-time tools. Nothing here ships in the app except the vendored HTMX file.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAILWIND_VERSION="v4.3.3"
HTMX_VERSION="4.0.0"
# Must match the pdfium-render API feature (`pdfium_7881`) in Cargo.toml.
PDFIUM_VERSION="7881"
PDFIUM_SHA256="52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40"
LLAMA_BUILD="b11349"
LLAMA_SHA256="bd0f0411541ad468959670ddd5ca8276135a9140cb978898286085aabe3f76f1"

mkdir -p "$ROOT/tools/bin" "$ROOT/apps/desktop/ui/vendor" "$ROOT/runtime/lib"

TW="$ROOT/tools/bin/tailwindcss"
if [[ ! -x "$TW" ]] || ! "$TW" --help 2>/dev/null | grep -q "${TAILWIND_VERSION#v}"; then
  echo "→ Tailwind CSS ${TAILWIND_VERSION} (standalone, macos-arm64)"
  curl -fsSL -o "$TW" \
    "https://github.com/tailwindlabs/tailwindcss/releases/download/${TAILWIND_VERSION}/tailwindcss-macos-arm64"
  chmod +x "$TW"
fi

HTMX="$ROOT/apps/desktop/ui/vendor/htmx.min.js"
if [[ ! -f "$HTMX" ]]; then
  echo "→ HTMX ${HTMX_VERSION}"
  curl -fsSL -o "$HTMX" "https://cdn.jsdelivr.net/npm/htmx.org@${HTMX_VERSION}/dist/htmx.min.js"
fi

PDFIUM="$ROOT/runtime/lib/libpdfium.dylib"
PDFIUM_STAMP="$ROOT/runtime/lib/.pdfium-$PDFIUM_VERSION"
if [[ ! -f "$PDFIUM" || ! -f "$PDFIUM_STAMP" ]]; then
  echo "→ PDFium chromium/${PDFIUM_VERSION} (mac-arm64)"
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  curl -fsSL -o "$TMP/pdfium.tgz" \
    "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${PDFIUM_VERSION}/pdfium-mac-arm64.tgz"
  echo "${PDFIUM_SHA256}  $TMP/pdfium.tgz" | shasum -a 256 -c --quiet
  tar -xzf "$TMP/pdfium.tgz" -C "$TMP"
  cp "$TMP/lib/libpdfium.dylib" "$PDFIUM"
  cp "$TMP/LICENSE" "$ROOT/runtime/lib/PDFIUM-LICENSE"
  rm -f "$ROOT"/runtime/lib/.pdfium-*
  touch "$PDFIUM_STAMP"
fi

LLAMA_DIR="$ROOT/runtime/llama"
LLAMA_STAMP="$LLAMA_DIR/.llama-$LLAMA_BUILD-closure"
if [[ ! -x "$LLAMA_DIR/llama-server" || ! -f "$LLAMA_STAMP" ]]; then
  echo "→ llama.cpp ${LLAMA_BUILD} llama-server (macos-arm64)"
  TMP_LLAMA="$(mktemp -d)"
  curl -fsSL -o "$TMP_LLAMA/llama.tgz" \
    "https://github.com/ggml-org/llama.cpp/releases/download/${LLAMA_BUILD}/llama-${LLAMA_BUILD}-bin-macos-arm64.tar.gz"
  echo "${LLAMA_SHA256}  $TMP_LLAMA/llama.tgz" | shasum -a 256 -c --quiet
  tar -xzf "$TMP_LLAMA/llama.tgz" -C "$TMP_LLAMA"
  SRC="$TMP_LLAMA/llama-${LLAMA_BUILD}"
  rm -rf "$LLAMA_DIR" && mkdir -p "$LLAMA_DIR"
  cp "$SRC/llama-server" "$LLAMA_DIR/"
  # llama-server resolves its dylibs via @rpath = @loader_path: copy exactly the dependency
  # closure, as real files under the names it asks for (no symlinks, nothing unused).
  copy_closure() {
    local deps
    deps="$(otool -L "$1" | awk 'NR > 1 { print $1 }' | sed -n 's|^@rpath/||p')"
    for dep in $deps; do
      if [[ ! -f "$LLAMA_DIR/$dep" ]]; then
        cp -L "$SRC/$dep" "$LLAMA_DIR/$dep"
        copy_closure "$LLAMA_DIR/$dep"
      fi
    done
  }
  copy_closure "$LLAMA_DIR/llama-server"
  cp "$SRC/LICENSE" "$LLAMA_DIR/LLAMA-CPP-LICENSE"
  rm -rf "$TMP_LLAMA"
  touch "$LLAMA_STAMP"
fi

# Sidecar, frameworks and licenses in the layout the Tauri bundler (and tauri-build) expect.
"$ROOT/scripts/stage-runtime.sh"

echo "✓ bootstrap ok"
