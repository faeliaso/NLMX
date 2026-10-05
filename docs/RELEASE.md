# Distribuição (macOS, Apple Silicon)

## Gerar o DMG

```sh
make bootstrap   # uma vez: Tailwind, HTMX, PDFium, llama.cpp (+ staging do runtime)
make bundle      # dist/NLMX.dmg (e dist/NLMX-<versão>.dmg), com verificação
```

O `make bundle`:

1. `scripts/stage-runtime.sh` copia o runtime de `runtime/` para o layout do bundler, em pastas ignoradas pelo git:
   - `src-tauri/binaries/llama-server-aarch64-apple-darwin`: o sidecar, com rpath `@executable_path/../Frameworks`;
   - `src-tauri/frameworks/*.dylib`: libllama, libggml*, libmtmd e libpdfium;
   - `src-tauri/licenses/`: PDFium, llama.cpp, sqlite-vec e SQLite.
2. `cargo tauri build --target aarch64-apple-darwin --bundles app,dmg`.
3. Copia o DMG para `dist/`.
4. `scripts/verify-bundle.sh` confere:
   - nenhum `.gguf` nem arquivo acima de 50 MB;
   - todo binário só `arm64`;
   - dependências só do bundle ou do sistema;
   - assinatura válida;
   - `--self-check` com ambiente vazio;
   - conteúdo do DMG.

## Conteúdo do app

```
NLMX.app/Contents/
  MacOS/nlmx-desktop      app Tauri/Rust (SQLite e sqlite-vec compilados dentro)
  MacOS/llama-server        sidecar de embeddings (llama.cpp b11349)
  Frameworks/               libpdfium (chromium/7881) + bibliotecas do llama.cpp/ggml (Metal)
  Resources/licenses/       licenças das bibliotecas incluídas
```

- **Modelos GGUF não vão no bundle.** São baixados pelo Model Manager (Modelos), sempre com confirmação, para `~/Library/Application Support/dev.nlmx.desktop/models/`.
- **Apple Foundation Models é do macOS** (`/usr/bin/fm`). O app exige macOS 27 e Apple Intelligence; a licença é aceita uma vez com `sudo fm license`.
- **Só Apple Silicon:** `LSRequiresNativeExecution` e `LSArchitecturePriority = arm64`. O app também recusa Rosetta em tempo de execução.

## Assinatura e notarização

Sem variáveis, o app sai **assinado ad-hoc** (`signingIdentity: "-"`) e **sem hardened runtime**: uma assinatura ad-hoc não tem Team ID, e a validação de bibliotecas do hardened runtime recusaria todas as dylibs de `Frameworks`. O `verify-bundle.sh` pegou isso no primeiro build. Ele funciona neste Mac; em outros, o Gatekeeper pede clique direito › Abrir na primeira vez.

Para distribuir de verdade:

```sh
export APPLE_SIGNING_IDENTITY="Developer ID Application: Nome (TEAMID)"
# notarização — Apple ID com senha de app…
export APPLE_ID="voce@exemplo.com" APPLE_PASSWORD="xxxx-xxxx-xxxx-xxxx" APPLE_TEAM_ID="TEAMID"
# …ou chave da API do App Store Connect
# export APPLE_API_ISSUER=… APPLE_API_KEY=… APPLE_API_KEY_PATH=…/AuthKey_XXXX.p8
make release
```

O Tauri assina o app, o sidecar e todas as bibliotecas de `Frameworks` com hardened runtime (`Entitlements.plist` vazio, sem exceções nem sandbox). Depois envia para notarização e grampeia o ticket. O `verify-bundle.sh` passa a exigir também `spctl --assess` e `stapler validate`.

## Onde o app guarda as coisas

| O quê | Caminho |
|---|---|
| banco, biblioteca de PDFs, índices | `~/Library/Application Support/dev.nlmx.desktop/` (`nlmx.sqlite3`, `library/`) |
| modelos baixados | `…/models/<id>/<versão>/` |
| logs JSON do app, log do llama-server | `…/logs/` (Configurações › Mostrar logs no Finder) |
| pidfiles, chave do llama-server, log do `fm serve` | `…/run/` |
| socket do `fm serve` | `$TMPDIR/nlmx-fm-<pid>-<n>.sock` |

Variáveis de desenvolvimento (`NLMX_PDFIUM_PATH`, `NLMX_LLAMA_SERVER`, `NLMX_EMBEDDING_CONFIG`) ainda funcionam como override. Os caminhos de fallback para `runtime/` e `NLMX_START_PATH` só existem em builds de debug.

## Atualizações

Não há atualizador automático nem verificação de rede: a única rede do app continua sendo o download de modelos pedido pelo usuário. Para atualizar, instale o DMG novo por cima (arraste para Aplicativos e substitua):

- **Dados mantidos:** documentos, conversas, índices e modelos ficam em Application Support e são mantidos;
- **Banco atualizado sozinho:** as migrações rodam ao abrir. São numeradas e reversíveis, e versões novas só acrescentam;
- **Servidor de embeddings substituído:** um `llama-server` deixado por uma versão anterior é trocado (o pidfile compara o binário).
- **Vindo do PDF RAG 0.1.0** (nome e bundle id antigos, `dev.pdfrag.desktop`): na primeira abertura a pasta de dados é movida para `dev.nlmx.desktop` (`src-tauri/src/legacy_data.rs`), com banco, logs e caminhos gravados atualizados. Uma cópia antiga do PDF RAG aberta depois não vê mais esses dados.

Configurações › Sobre mostra a versão. Com `NLMX_DOWNLOAD_URL=https://…` definido no build, aparece também o botão **Página de downloads**.

## Antes de publicar

- [ ] `make lint && make test`
- [ ] Versão atualizada em `apps/desktop/src-tauri/tauri.conf.json` e `Cargo.toml` do workspace
- [ ] `make release` (ou `make bundle` para testes) sem falhas na verificação
- [ ] `make acceptance`: instalação limpa, primeira execução, download + checksum do modelo, uso offline (RAG, chat, citações, viewer) com o runtime do app instalado
- [ ] Instalar o DMG num Mac limpo, sem o repositório: importar um PDF, baixar o modelo em Modelos, perguntar no Chat
- [ ] Registrar o resultado em `docs/releases/<versão>.md` (modelo: `releases/0.1.0.md`)
