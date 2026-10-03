#!/usr/bin/env bash
# Prepares the native runtime for the app bundle (run by `make bundle`):
#   runtime/llama/llama-server → apps/desktop/src-tauri/binaries/llama-server-aarch64-apple-darwin
#                                (Tauri sidecar → Contents/MacOS/llama-server)
#   runtime/llama/*.dylib, runtime/lib/libpdfium.dylib → apps/desktop/src-tauri/frameworks/
#                                (→ Contents/Frameworks)
#   licenses → apps/desktop/src-tauri/licenses/ (→ Contents/Resources/licenses)
# runtime/ itself is left untouched for development.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAURI="$ROOT/apps/desktop/src-tauri"
TRIPLE="aarch64-apple-darwin"

[[ -x "$ROOT/runtime/llama/llama-server" && -f "$ROOT/runtime/lib/libpdfium.dylib" ]] \
  || { echo "runtime/ incompleto — rode \`make bootstrap\`" >&2; exit 1; }

rm -rf "$TAURI/binaries" "$TAURI/frameworks" "$TAURI/licenses"
mkdir -p "$TAURI/binaries" "$TAURI/frameworks" "$TAURI/licenses"

# Sidecar: finds its libraries in ../Frameworks once bundled.
SIDECAR="$TAURI/binaries/llama-server-$TRIPLE"
cp "$ROOT/runtime/llama/llama-server" "$SIDECAR"
install_name_tool -add_rpath "@executable_path/../Frameworks" "$SIDECAR" 2>/dev/null || true

# Libraries (they reference each other through @rpath + @loader_path).
cp "$ROOT"/runtime/llama/*.dylib "$TAURI/frameworks/"
cp "$ROOT/runtime/lib/libpdfium.dylib" "$TAURI/frameworks/"

# install_name_tool invalidates signatures: re-sign ad-hoc (the bundler signs again for release).
for f in "$SIDECAR" "$TAURI"/frameworks/*.dylib; do
  codesign --force --sign - --timestamp=none "$f" >/dev/null 2>&1
done

# Licenses of what ships in the app.
cp "$ROOT/runtime/lib/PDFIUM-LICENSE" "$TAURI/licenses/PDFium-LICENSE.txt"
cp "$ROOT/runtime/llama/LLAMA-CPP-LICENSE" "$TAURI/licenses/llama.cpp-LICENSE.txt"
# The sqlite-vec crate ships no license file; its manifest declares MIT/Apache-2.0.
cat > "$TAURI/licenses/sqlite-vec.txt" <<'TXT'
sqlite-vec (https://github.com/asg017/sqlite-vec) — Copyright (c) Alex Garcia.
Dual-licensed under the MIT License or the Apache License 2.0, at your option.
TXT
cat > "$TAURI/licenses/SQLite.txt" <<'TXT'
SQLite is in the public domain (https://sqlite.org/copyright.html).
TXT

echo "runtime staged: $(ls "$TAURI/frameworks" | wc -l | tr -d ' ') libraries, sidecar $(basename "$SIDECAR")"
