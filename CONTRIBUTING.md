# Contribuindo com o NLMX

Obrigado pelo interesse em contribuir! Este guia reúne o que você precisa saber antes de abrir uma issue ou um pull request.

## Preparar o ambiente

Siga os [Requisitos](README.md#requisitos) e os [Primeiros passos](README.md#primeiros-passos) do README. Em resumo:

```sh
make bootstrap
make dev
```

Para os testes com modelos reais, baixe o modelo de embeddings (`./scripts/fetch-embedding-model.sh`) e aceite a licença do `fm` (`sudo fm license`).

## Fluxo de trabalho

1. Crie uma branch a partir de `develop` (`feature/…`, `fix/…`, `docs/…`).
2. Faça mudanças pequenas e focadas, com testes.
3. Abra o pull request para `develop`. A branch `main` recebe só releases.

Antes de abrir o PR, rode:

```sh
make lint   # cargo fmt --check + clippy -D warnings
make test   # todos os testes sem modelo real
```

Se a mudança afeta RAG, embeddings ou a geração, rode também `make test-real`. As suítes estão descritas em [`docs/TESTING.md`](docs/TESTING.md).

## Regras do código

### Arquitetura

- O workspace é hexagonal ([ADR 0001](docs/adr/0001-hexagonal-cargo-workspace.md)): `domain` ← `application` ← `adapters/*` e `ui-web` ← `apps/desktop/src-tauri`.
- `application` não depende de crates de infraestrutura. Adapters não dependem uns dos outros, e `ui-web` não depende de adapters nem do Tauri.
- Tipos externos não atravessam ports: adapters convertem para tipos do `domain`.
- Todo port tem um fake em memória e uma suíte de contrato em `crates/testing/`, e todo adapter novo precisa passar nessa suíte.
- `tests/tests/architecture.rs` verifica essas regras e precisa continuar passando.

### Privacidade

- Documentos e perguntas nunca saem do dispositivo. Não adicione acesso à rede além do download de modelos iniciado pelo usuário.
- **Nunca registre em log** texto de documentos, trechos, perguntas, respostas, prompts, títulos, nomes de arquivo ou caminhos do usuário. Identifique documentos pelo id.
- Medições passam por `telemetry::record(Measurement)`. Para medir algo novo, adicione uma variante em vez de registrar números avulsos.
- O teste `privacy_canary` precisa continuar passando.

### Banco de dados

- Migrations ficam em `crates/adapters/store-sqlite/migrations/NNNN_nome/` e sempre têm `up.sql` e `down.sql`.
- Use tabelas `STRICT`, timestamps ISO-8601 com milissegundos e colunas de status com `CHECK`.
- Toda tabela nova com dados derivados de documentos precisa ser coberta pela remoção de documento ([ADR 0008](docs/adr/0008-remocao-de-documento.md)).

### Interface

- Use só os tokens de `apps/desktop/ui/styles/tokens.css`, sem cores cruas nem a variante `dark:` do Tailwind. Veja [`docs/DESIGN_SYSTEM.md`](docs/DESIGN_SYSTEM.md).
- Texto gerado pelo modelo é renderizado apenas por `crates/ui-web/src/markdown.rs`.

## Decisões e documentação

- Para mudar uma decisão registrada, crie um novo ADR em [`docs/adr/`](docs/adr/). Não reescreva ADRs existentes; acrescente uma seção "Atualização" ou crie um ADR novo.
- Atualize a documentação em `docs/` quando a mudança alterar o comportamento, e o status dos requisitos em [`docs/PRODUCT.md`](docs/PRODUCT.md).

## Commits

- Mensagens curtas, no imperativo, descrevendo o que a mudança faz (por exemplo: `Free llama-server memory after indexing`).
- Um assunto por commit sempre que possível.

## Reportando bugs

Abra uma issue com:

- a versão do NLMX e do macOS, e o modelo do Mac;
- os passos para reproduzir, o comportamento esperado e o obtido;
- se relevante, trechos de `~/Library/Application Support/dev.nlmx.desktop/logs/nlmx.jsonl`.

**Não anexe documentos privados.** Se o problema depende de um PDF específico, tente reproduzi-lo com um arquivo público ou gerado para o teste.

## Licença

Ao contribuir, você concorda que suas contribuições serão licenciadas sob a [licença MIT](LICENSE) do projeto.
