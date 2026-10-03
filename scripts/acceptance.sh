#!/usr/bin/env bash
# Release acceptance of the built DMG (run `make bundle` first). Nothing touches /Applications or
# the user's data: the app is installed into a temporary folder and runs with a temporary HOME.
#   1. clean install from the DMG (+ quarantine flag, Gatekeeper assessment)
#   2. first launch of the GUI app (fresh HOME): window, database, logs; quit via Apple Event
#   3. installed binaries: --self-check with an empty environment, llama-server --version
#   4. no Ollama, no Python in the bundle
#   5. tests/tests/release_acceptance.rs against the installed app's runtime: model download +
#      checksum, then offline ingestion, RAG with Apple FM, chat, citations, viewer
# Output: target/acceptance/{acceptance.log,report.json}
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DMG="${1:-$ROOT/dist/NLMX.dmg}"
OUT="$ROOT/target/acceptance"
mkdir -p "$OUT"
LOG="$OUT/acceptance.log"
: > "$LOG"
FAILED=0
say() { printf "%s\n" "$1" | tee -a "$LOG"; }
pass() { say "  ✓ $1"; }
fail() { say "  ✗ $1"; FAILED=1; }
note() { say "    $1"; }

[[ -f "$DMG" ]] || { echo "DMG não encontrado: $DMG (rode make bundle)"; exit 1; }
WORK=$(mktemp -d /tmp/nlmx-acceptance.XXXX)
INSTALL="$WORK/Applications"
FAKE_HOME="$WORK/home"
mkdir -p "$INSTALL" "$FAKE_HOME"
APP="$INSTALL/NLMX.app"
DATA="$FAKE_HOME/Library/Application Support/dev.nlmx.desktop"

say "1. Instalação limpa a partir do DMG ($(du -h "$DMG" | cut -f1))"
MNT="$WORK/dmg"
mkdir -p "$MNT"
if hdiutil attach -nobrowse -readonly -mountpoint "$MNT" "$DMG" >/dev/null 2>&1; then
  ditto "$MNT/NLMX.app" "$APP" && pass "app copiado do DMG para uma pasta Aplicativos temporária"
  hdiutil detach "$MNT" >/dev/null 2>&1
else
  fail "não foi possível montar o DMG"; exit 1
fi
note "tamanho instalado: $(du -sh "$APP" | cut -f1)"
xattr -w com.apple.quarantine "0081;$(printf %x "$(date +%s)");Safari;" "$APP"
if spctl --assess --type execute "$APP" 2>/dev/null; then
  pass "Gatekeeper aceita o app baixado (assinado e notarizado)"
else
  note "Gatekeeper recusa o app baixado: esperado sem Developer ID/notarização (abrir com clique direito › Abrir)"
fi
xattr -d com.apple.quarantine "$APP"

say "2. Primeira execução (HOME vazio)"
open -n -g --env HOME="$FAKE_HOME" --env TMPDIR="$WORK/" "$APP"
for _ in $(seq 1 60); do
  grep -q "main window created" "$DATA/logs/nlmx.jsonl" 2>/dev/null && break
  sleep 0.5
done
PID=$(pgrep -f "$APP/Contents/MacOS/nlmx-desktop" | head -1)
[[ -n "$PID" ]] && pass "app aberto (pid $PID)" || fail "o app não abriu"
grep -q "main window created" "$DATA/logs/nlmx.jsonl" 2>/dev/null && pass "janela principal criada" || fail "janela não criada (veja $DATA/logs)"
schema=$(/usr/bin/sqlite3 "$DATA/nlmx.sqlite3" 'PRAGMA user_version' 2>/dev/null)
[[ "$schema" -ge 1 ]] 2>/dev/null && pass "banco criado e migrado (schema $schema) em Application Support" || fail "banco não criado"
if [[ -s "$DATA/logs/nlmx.jsonl" ]]; then
  /usr/bin/python3 -c "import json,sys;[json.loads(l) for l in open(sys.argv[1])]" "$DATA/logs/nlmx.jsonl" 2>/dev/null \
    && pass "logs JSON estruturados ($(wc -l < "$DATA/logs/nlmx.jsonl" | tr -d ' ') linhas)" || fail "logs não são JSON válido"
  grep -qE '/Users/|/var/folders/|/private/' "$DATA/logs/nlmx.jsonl" && fail "logs contêm caminhos do usuário" || pass "logs sem caminhos do usuário"
fi
osascript -e "tell application \"$APP\" to quit" >/dev/null 2>&1
for _ in $(seq 1 30); do kill -0 "$PID" 2>/dev/null || break; sleep 0.5; done
kill -0 "$PID" 2>/dev/null && { fail "o app não encerrou pelo Quit"; kill "$PID"; } || pass "encerrado pelo menu Sair (Apple Event)"
leftovers=$(pgrep -fl "$WORK" | grep -E 'llama-server|fm serve' || true)
[[ -z "$leftovers" ]] && pass "nenhum processo auxiliar deixado para trás" || fail "processos deixados: $leftovers"

say "3. Binários instalados"
report=$(env -i HOME="$FAKE_HOME" PATH=/usr/bin:/bin TMPDIR="$WORK/" "$APP/Contents/MacOS/nlmx-desktop" --self-check 2>/dev/null)
if [[ $? == 0 ]]; then
  pass "self-check com ambiente vazio"
  echo "$report" > "$OUT/self-check.json"
  /usr/bin/python3 -c '
import json,sys; r=json.load(open(sys.argv[1]))
print("    PDFium %s · SQLite %s · sqlite-vec %s · llama.cpp %s (do app: %s) · Apple FM: %s" % (
  r["pdfium"]["build"], r["database"]["sqlite"], r["database"]["sqlite_vec"],
  r["llama_cpp"]["build"], r["llama_cpp"]["bundled"], r["apple_foundation_models"]))' "$OUT/self-check.json" | tee -a "$LOG"
else
  fail "self-check falhou: $report"
fi
"$APP/Contents/MacOS/llama-server" --version >/dev/null 2>&1 && pass "llama-server do app executa" || fail "llama-server do app não executa"

say "4. Sem Ollama, sem Python"
machos=$(find "$APP/Contents" -type f \( -perm -u+x -o -name '*.dylib' \) -exec sh -c 'file -b "$1" | grep -q Mach-O && echo "$1"' _ {} \;)
ollama_refs=$(echo "$machos" | while read -r f; do strings "$f" | grep -il 'ollama' >/dev/null && echo "$f"; done)
[[ -z "$ollama_refs" ]] && pass "nenhuma referência a Ollama nos binários" || fail "referências a Ollama: $ollama_refs"
py_links=$(echo "$machos" | while read -r f; do otool -L "$f" | grep -i python; done)
py_files=$(find "$APP" -name '*.py' -o -name '*.pyc' -o -iname 'Python.framework')
[[ -z "$py_links$py_files" ]] && pass "nenhum Python embutido ou ligado" || fail "Python no bundle: $py_links $py_files"
note "o self-check acima rodou com PATH=/usr/bin:/bin (sem Homebrew, sem Ollama)"

say "5. Modelo, offline, RAG, chat, citações e viewer (runtime do app instalado)"
note "baixando o modelo de embeddings (~640 MB) pelo Model Manager…"
if ACCEPTANCE_APP="$APP" \
   NLMX_PDFIUM_PATH="$APP/Contents/Frameworks/libpdfium.dylib" \
   NLMX_LLAMA_SERVER="$APP/Contents/MacOS/llama-server" \
   cargo test --release -p nlmx-workspace-tests --test release_acceptance -- --ignored --nocapture >"$OUT/release_acceptance.log" 2>&1; then
  pass "aceitação de ponta a ponta"
else
  fail "aceitação de ponta a ponta (veja target/acceptance/release_acceptance.log)"
  grep -E 'panicked|✗' "$OUT/release_acceptance.log" | head -5 | tee -a "$LOG"
fi
if [[ -f "$OUT/report.json" ]]; then
  /usr/bin/python3 - "$OUT/report.json" <<'PY' | tee -a "$LOG"
import json, sys
r = json.load(open(sys.argv[1]))
m = r["model"]; g = r["rag"]; n = r["network"]; s = r["metrics"]
print(f"    modelo: {m['id']} · {m['bytes']/1e6:.0f} MB em {m['download_secs']:.0f} s · checksum ok · corrupção detectada")
print(f"    RAG: citações válidas {g['citations_valid']}/{g['answerable']} · 'não encontrado' {g['not_found_correct']}/{g['unanswerable']}")
print(f"    chat: {len(r['chat'])} perguntas respondidas com citação · viewer: página {r['viewer']['page']}, {r['viewer']['search_hits']} resultados de busca")
print(f"    rede durante o uso: {', '.join(n['remotes_seen']) or 'nenhuma'} · externas: {', '.join(n['external']) or 'nenhuma'}")
ops = {o['operation']: o for o in s['operations']}
print(f"    importação {r['ingestion_secs']:.1f} s · {s['pages_per_second'] or 0:.0f} páginas/s · {s['embeddings_per_second'] or 0:.1f} embeddings/s · "
      f"retrieval p50 {ops['retrieve']['p50_ms']} ms · geração p50 {ops['generate']['p50_ms']} ms")
PY
fi

rm -rf "$WORK"
[[ $FAILED == 0 ]] && say "✓ aceitação ok" || { say "✗ aceitação com falhas"; exit 1; }
