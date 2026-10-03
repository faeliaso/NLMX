#!/usr/bin/env bash
# Downloads the default multilingual embedding model (~640 MB) into models/ for development and
# for `make test-llama`. End users get models through the app's model manager instead.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO="Qwen/Qwen3-Embedding-0.6B-GGUF"
REVISION="370f27d7550e0def9b39c1f16d3fbaa13aa67728"
FILE="Qwen3-Embedding-0.6B-Q8_0.gguf"
SHA256="06507c7b42688469c4e7298b0a1e16deff06caf291cf0a5b278c308249c3e439"

DEST="$ROOT/models/$FILE"
if [[ -f "$DEST" ]] && echo "${SHA256}  $DEST" | shasum -a 256 -c --quiet 2>/dev/null; then
  echo "✓ $FILE already present"
else
  echo "→ $REPO/$FILE (Apache-2.0)"
  curl -fL --progress-bar -C - -o "$DEST.part" "https://huggingface.co/$REPO/resolve/$REVISION/$FILE"
  echo "${SHA256}  $DEST.part" | shasum -a 256 -c --quiet
  mv "$DEST.part" "$DEST"
fi

CONFIG="$ROOT/models/embedding.json"
if [[ ! -f "$CONFIG" ]]; then
  cp "$ROOT/models/embedding.example.json" "$CONFIG"
  echo "✓ wrote models/embedding.json (use it with NLMX_EMBEDDING_CONFIG)"
fi
