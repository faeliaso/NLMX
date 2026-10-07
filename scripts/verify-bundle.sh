#!/usr/bin/env bash
# Checks a built NLMX.app (and its DMG) before distribution:
#   1. no models (no *.gguf, no file > 50 MB) in the app or the DMG
#   2. every Mach-O is arm64 only
#   3. every linked library resolves inside the bundle or to the system (no Homebrew, no workspace)
#   4. code signature is valid (ad-hoc or Developer ID; Gatekeeper/notarization when signed)
#   5. the app runs on its own: `--self-check` with an empty environment and a temporary HOME
#   6. the DMG contains the app and the Applications shortcut
# Usage: scripts/verify-bundle.sh <path/to/NLMX.app> [path/to/NLMX.dmg]
set -uo pipefail

APP="${1:?usage: verify-bundle.sh <app> [dmg]}"
DMG="${2:-}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FAILED=0
pass() { printf "  ✓ %s\n" "$1"; }
fail() { printf "  ✗ %s\n" "$1"; FAILED=1; }

echo "Verificando $(basename "$APP")"

# 1. No models.
big=$(find "$APP" -type f \( -name '*.gguf' -o -size +50M \) -print)
[[ -z "$big" ]] && pass "sem modelos nem arquivos > 50 MB no app" || fail "arquivos grandes/modelos no app: $big"
echo "    tamanho do app: $(du -sh "$APP" | cut -f1)"

# Mach-O files of the bundle.
machos=()
while IFS= read -r f; do
  file -b "$f" | grep -q 'Mach-O' && machos+=("$f")
done < <(find "$APP/Contents" -type f \( -perm -u+x -o -name '*.dylib' \))

# 2. Architecture.
bad_arch=0
for f in "${machos[@]}"; do
  archs=$(lipo -archs "$f" 2>/dev/null)
  [[ "$archs" == "arm64" ]] || { echo "    $f: $archs"; bad_arch=1; }
done
[[ $bad_arch == 0 ]] && pass "${#machos[@]} binários, todos só arm64" || fail "binários que não são só arm64"

# 3. Linked libraries.
bad_link=0
for f in "${machos[@]}"; do
  while read -r dep _; do
    case "$dep" in
      /System/*|/usr/lib/*|@rpath/*|@loader_path/*|@executable_path/*) ;;
      ./libpdfium.dylib) ;; # PDFium's own install name; it is loaded by path from Frameworks
      *) echo "    $(basename "$f") → $dep"; bad_link=1 ;;
    esac
  done < <(otool -L "$f" | tail -n +2)
  # @rpath entries must point inside the bundle.
  while read -r rpath; do
    case "$rpath" in
      @loader_path*|@executable_path*) ;;
      *) echo "    $(basename "$f") rpath → $rpath"; bad_link=1 ;;
    esac
  done < <(otool -l "$f" | awk '/LC_RPATH/{getline; getline; print $2}')
  strings "$f" | grep -q "$ROOT" && { echo "    $(basename "$f") contém o caminho do projeto"; }
done
for lib in $(otool -L "$APP/Contents/MacOS/llama-server" | awk '/@rpath/{print $1}' | sed 's|@rpath/||'); do
  [[ -f "$APP/Contents/Frameworks/$lib" ]] || { echo "    falta Frameworks/$lib"; bad_link=1; }
done
[[ $bad_link == 0 ]] && pass "dependências resolvem dentro do bundle ou no sistema" || fail "dependências externas"

# 4. Signature.
if codesign --verify --deep --strict "$APP" 2>/tmp/nlmx-codesign.err; then
  authority=$(codesign -dvv "$APP" 2>&1 | awk -F= '/^Authority/{print $2; exit}')
  pass "assinatura válida (${authority:-ad-hoc})"
  if [[ -n "$authority" ]]; then
    spctl --assess --type execute "$APP" 2>/dev/null && pass "Gatekeeper aceita" || fail "Gatekeeper recusou (notarização?)"
    xcrun stapler validate "$APP" >/dev/null 2>&1 && pass "ticket de notarização grampeado" || fail "sem ticket de notarização"
  else
    echo "    ad-hoc: em outros Macs, abrir com clique direito › Abrir na primeira vez"
  fi
else
  fail "assinatura inválida: $(cat /tmp/nlmx-codesign.err)"
fi

# 5. Runs on its own.
TMP_HOME=$(mktemp -d)
report=$(env -i HOME="$TMP_HOME" PATH=/usr/bin:/bin TMPDIR="$TMP_HOME/" "$APP/Contents/MacOS/nlmx-desktop" --self-check 2>/dev/null)
status=$?
if [[ $status == 0 ]]; then
  pass "self-check sem ambiente (PDFium, SQLite + sqlite-vec, llama.cpp do bundle)"
  echo "$report" | /usr/bin/python3 -c '
import json,sys; r=json.load(sys.stdin)
print("    PDFium", r["pdfium"].get("build"), "· SQLite", r["database"].get("sqlite"), "· sqlite-vec", r["database"].get("sqlite_vec"))
print("    llama.cpp", r["llama_cpp"].get("build"), "· incluído no app:", r["llama_cpp"].get("bundled"))
print("    Apple FM:", r["apple_foundation_models"], "· licenças:", ", ".join(r["licenses"] or []))
print("    build de debug!" if r["debug_build"] else "    build release")'
  echo "$report" | grep -q '"bundled": true' || fail "llama-server não veio do bundle"
else
  fail "self-check falhou (código $status): $report"
fi
"$APP/Contents/MacOS/llama-server" --version >/dev/null 2>&1 && pass "llama-server --version" || fail "llama-server não executa"
rm -rf "$TMP_HOME"

# 6. DMG.
if [[ -n "$DMG" ]]; then
  echo "    DMG: $(basename "$DMG") $(du -h "$DMG" | cut -f1)"
  MNT=$(mktemp -d)
  if hdiutil attach -nobrowse -readonly -mountpoint "$MNT" "$DMG" >/dev/null 2>&1; then
    [[ -d "$MNT/NLMX.app" ]] && pass "DMG contém NLMX.app" || fail "DMG sem o app"
    [[ -L "$MNT/Applications" ]] && pass "DMG tem o atalho Aplicativos" || fail "DMG sem o atalho Aplicativos"
    dmg_big=$(find "$MNT" -type f \( -name '*.gguf' -o -size +50M \) -print 2>/dev/null)
    [[ -z "$dmg_big" ]] && pass "sem modelos no DMG" || fail "arquivos grandes no DMG: $dmg_big"
    hdiutil detach "$MNT" >/dev/null 2>&1
  else
    fail "não foi possível montar o DMG"
  fi
  rmdir "$MNT" 2>/dev/null
fi

[[ $FAILED == 0 ]] && echo "✓ bundle verificado" || { echo "✗ verificação falhou"; exit 1; }
