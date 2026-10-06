<p align="center">
  <img src="apps/desktop/src-tauri/icons/128x128@2x.png" width="128" alt="Ícone do NLMX">
</p>

<h1 align="center">NLMX</h1>

<p align="center">
  Pergunte aos seus PDFs, com tudo rodando no seu Mac.
</p>

<div align="center">

![macOS 27+](https://img.shields.io/badge/macOS-27%2B-000000?logo=apple&logoColor=white) ![Apple Silicon](https://img.shields.io/badge/Apple%20Silicon-arm64-555555?logo=apple&logoColor=white) ![Rust 1.85+](https://img.shields.io/badge/Rust-1.85%2B-B7410E?logo=rust&logoColor=white) ![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white) ![Versão 0.1.0](https://img.shields.io/badge/vers%C3%A3o-0.1.0-blue) ![Licença MIT](https://img.shields.io/badge/licen%C3%A7a-MIT-green) ![100% local](https://img.shields.io/badge/100%25-local-2ea44f)

</div>

---

## Sobre o projeto

**NLMX** é um app nativo para macOS que transforma uma coleção de PDFs numa base de conhecimento consultável em linguagem natural. Você importa os documentos, faz perguntas e recebe respostas fundamentadas, com **citações clicáveis** que levam à página e ao trecho exato do PDF.

Tudo roda no próprio Mac: a extração, a indexação, a busca e a geração das respostas. Documentos, índices e perguntas nunca saem do dispositivo, e o app funciona offline depois do download do modelo de embeddings. Quando não encontra a resposta nos documentos, o app diz isso em vez de inventar uma.

Princípios:

- **Local-first de verdade**: funciona offline depois do download do modelo.
- **Zero atrito de instalação**: um `.dmg`, sem Python, Ollama, Docker ou terminal.
- **Confiança verificável**: toda resposta mostra de onde veio.
- **Nativo do macOS**: usa o modelo do sistema (Apple Foundation Models) em vez de embarcar um LLM gerador.

## Funcionalidades

- **Biblioteca de PDFs**: importação com detecção de duplicatas por SHA-256 e cópia para uma biblioteca interna.
- **Extração e estrutura**: texto por página com bounding boxes (PDFium), remoção de cabeçalhos e rodapés repetidos, junção de palavras hifenizadas, títulos e seções.
- **Busca híbrida**: BM25 (FTS5) combinado com busca vetorial (sqlite-vec) por fusão ponderada.
- **Chat com citações**: respostas em streaming pelo Apple Foundation Models, com fontes `[n]` e referências `[página N]` clicáveis; as conversas ficam salvas.
- **Visualizador de PDF** ao lado do chat, com destaque do trecho citado, seleção de texto e busca.
- **Modelos**: download de modelos de embeddings sob confirmação explícita, com progresso, verificação de checksum, ativação e remoção.
- **Remoção sem rastro**: remover um documento apaga todos os dados derivados dele, inclusive o histórico de chat que o usou ([ADR 0008](docs/adr/0008-remocao-de-documento.md)).

Ainda não implementado: OCR de PDFs digitalizados. O status de cada requisito está em [`docs/PRODUCT.md`](docs/PRODUCT.md).

## Privacidade

- Documentos e perguntas **nunca saem do dispositivo**. O único acesso à rede é o download de um modelo, sempre iniciado pelo usuário.
- O app não abre portas TCP. A única exceção é o `llama-server` supervisionado, que escuta só em `127.0.0.1`, numa porta efêmera e com chave por execução ([ADR 0006](docs/adr/0006-llama-server-loopback.md)).
- Os logs nunca contêm texto de documentos, perguntas, respostas, títulos ou caminhos. Um teste canário garante isso.

## Tecnologias

| Camada | Tecnologia |
|---|---|
| App desktop | [Tauri 2](https://tauri.app) + Rust (edition 2024), workspace com arquitetura hexagonal ([ADR 0001](docs/adr/0001-hexagonal-cargo-workspace.md)) |
| Interface | Templates [askama](https://github.com/askama-rs/askama), router [axum](https://github.com/tokio-rs/axum) em processo via protocolo `nlmx://` ([ADR 0003](docs/adr/0003-in-process-router-custom-protocol.md)), [HTMX 4](https://htmx.org) vendorizado e [Tailwind CSS 4](https://tailwindcss.com) compilado no build |
| PDF | [PDFium](https://pdfium.googlesource.com/pdfium/) (build 7881) via [pdfium-render](https://github.com/ajrcarey/pdfium-render) |
| Armazenamento e busca | [SQLite](https://sqlite.org) com FTS5 + [sqlite-vec](https://github.com/asg017/sqlite-vec), em um único banco ([ADR 0004](docs/adr/0004-single-sqlite-store-fts5-sqlite-vec.md), [0005](docs/adr/0005-embedding-space-per-model.md), [0007](docs/adr/0007-weighted-hybrid-fusion.md)) |
| Embeddings | [llama.cpp](https://github.com/ggml-org/llama.cpp) (`llama-server` b11349) com Qwen3-Embedding-0.6B (GGUF) |
| Geração | Apple Foundation Models pelo CLI do sistema `/usr/bin/fm`, sem Swift ([ADR 0002](docs/adr/0002-fm-cli-serve-over-uds.md)) |

## Requisitos

**Para usar o app**

- macOS 27 ou superior em Apple Silicon (sem suporte a Intel ou Rosetta)
- Apple Intelligence ativo
- A licença do `fm` aceita uma vez no Terminal com `sudo fm license`. O app nunca aceita essa licença no lugar do usuário.

**Para desenvolver**

- Rust stable (fixado em [`rust-toolchain.toml`](rust-toolchain.toml), alvo `aarch64-apple-darwin`)
- Tauri CLI: `cargo install tauri-cli`
- Xcode Command Line Tools: `xcode-select --install`
- ImageMagick, só para regenerar os ícones

## Primeiros passos

```sh
git clone <url-do-repositório> NLMX
cd NLMX

make bootstrap                        # Tailwind CLI, HTMX, PDFium e llama-server (uma vez)
./scripts/fetch-embedding-model.sh    # opcional: modelo de embeddings para desenvolvimento (~640 MB)
make dev                              # roda o app em modo debug
```

Para usar o modelo baixado pelo script sem passar pela tela Modelos:

```sh
NLMX_EMBEDDING_CONFIG=$PWD/models/embedding.json make dev
```

## Scripts

| Comando | O que faz |
|---|---|
| `make bootstrap` | Baixa as ferramentas de build e o runtime: Tailwind CLI, HTMX 4, PDFium e `llama-server` |
| `make dev` | Roda o app em modo debug (`cargo tauri dev`) |
| `make build` | Compila o workspace |
| `make test` | Todos os testes que não precisam de modelo real |
| `make test-unit` · `test-integration` | Subconjuntos do `make test` |
| `make lint` · `make fmt` | `cargo fmt --check` + `clippy -D warnings` · formatação |
| `make bundle` | Gera `dist/NLMX.dmg` (assinatura ad-hoc) e verifica o bundle |
| `make release` | Gera o DMG assinado com Developer ID e notarizado |
| `make acceptance` | Instala o DMG num ambiente temporário e o verifica de ponta a ponta |
| `./scripts/fetch-embedding-model.sh` | Baixa o modelo de embeddings de desenvolvimento para `models/` |
| `./scripts/make-icons.sh` | Regenera os ícones do app a partir de `icons/source.png` |

Mais detalhes em [`docs/TESTING.md`](docs/TESTING.md) e [`docs/RELEASE.md`](docs/RELEASE.md).

## Instalação

1. Gere o instalador com `make bundle`, ou use um `NLMX.dmg` publicado.
2. Abra o DMG e arraste o **NLMX** para **Aplicativos**.
3. Builds com assinatura ad-hoc precisam ser abertas pela primeira vez com clique direito › **Abrir**.
4. Na tela **Modelos**, baixe o modelo de embeddings. Depois disso, o app funciona offline.

Os dados do app ficam em `~/Library/Application Support/dev.nlmx.desktop/`.

## Estrutura do repositório

```
apps/desktop/          app Tauri: src-tauri/ (composition root, comandos) e ui/ (templates, estilos, scripts)
crates/
  domain/              tipos e regras puras
  application/         casos de uso e todos os ports
  adapters/            implementações: pdf-pdfium, store-sqlite, embed-llama, llm-fm, models-catalog, …
  ui-web/              router axum e renderização das páginas
  testing/             fakes em memória e suítes de contrato dos ports
tests/                 testes de arquitetura, ativação de modelo e aceitação de release
scripts/               bootstrap, bundle, verificação, aceitação e ícones
docs/                  produto, arquitetura, design system, testes, release e ADRs
```

## Documentação

| Documento | Conteúdo |
|---|---|
| [`docs/PRODUCT.md`](docs/PRODUCT.md) | Visão, requisitos com status, metas e riscos |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Camadas, ports, fluxos, processos e banco |
| [`docs/DESIGN_SYSTEM.md`](docs/DESIGN_SYSTEM.md) | Tokens, componentes e acessibilidade |
| [`docs/TESTING.md`](docs/TESTING.md) | Suítes de teste, métricas e logs |
| [`docs/RELEASE.md`](docs/RELEASE.md) | DMG, assinatura, notarização e checklist |
| [`docs/adr/`](docs/adr/) | Decisões de arquitetura |

## Contribuição

Contribuições são bem-vindas! Consulte o arquivo [CONTRIBUTING.md](CONTRIBUTING.md) para obter as diretrizes.

## Licença

Distribuído sob a licença MIT. Veja [LICENSE](LICENSE).

O app inclui componentes de terceiros com licenças próprias (PDFium, llama.cpp, SQLite e sqlite-vec). Os textos dessas licenças estão em [`apps/desktop/src-tauri/licenses/`](apps/desktop/src-tauri/licenses/) e acompanham o app instalado.
