# Contributing to NLMX

Thanks for your interest in contributing! This guide covers what you need to know before opening an issue or a pull request.

## Setting up the environment

Follow the [Requirements](README.md#requirements) and [Quick Start](README.md#quick-start) sections of the README (also available in [Portuguese](README.pt-BR.md)). In short:

```sh
make bootstrap
make dev
```

For tests with real models, download the embedding model (`./scripts/fetch-embedding-model.sh`) and accept the `fm` license (`sudo fm license`).

## Workflow

1. Create a branch from `develop` (`feature/…`, `fix/…`, `docs/…`).
2. Make small, focused changes, with tests.
3. Open the pull request against `develop`. The `main` branch only receives releases.

Before opening the PR, run:

```sh
make lint   # cargo fmt --check + clippy -D warnings
make test   # all tests that need no real model
```

There are only unit and integration tests (`make test`). The suites are described in [`docs/TESTING.md`](docs/TESTING.md).

## Code rules

### Architecture

- The workspace is hexagonal ([ADR 0001](docs/adr/0001-hexagonal-cargo-workspace.md)): `domain` ← `application` ← `adapters/*` and `ui-web` ← `apps/desktop/src-tauri`.
- `application` does not depend on infrastructure crates. Adapters do not depend on each other, and `ui-web` depends on neither adapters nor Tauri.
- External types do not cross ports: adapters convert them to `domain` types.
- Every port has an in-memory fake and a contract suite in `crates/testing/`, and every new adapter must pass that suite.
- `tests/tests/architecture.rs` checks these rules and must keep passing.

### Privacy

- Documents and questions never leave the device. Do not add network access beyond the user-initiated model download.
- **Never log** document text, passages, questions, answers, prompts, titles, file names or user paths. Identify documents by id.
- Measurements go through `telemetry::record(Measurement)`. To measure something new, add a variant instead of logging ad hoc numbers.
- There is no longer an automated privacy test: review any change to logs and metrics by hand.

### Database

- Migrations live in `crates/adapters/store-sqlite/migrations/NNNN_name/` and always have `up.sql` and `down.sql`.
- Use `STRICT` tables, ISO-8601 timestamps with milliseconds, and `CHECK`ed status columns.
- Every new table holding document-derived data must be covered by document removal ([ADR 0008](docs/adr/0008-document-removal.md)).

### Interface

- Use only the tokens in `apps/desktop/ui/styles/tokens.css`, with no raw colors and no Tailwind `dark:` variant. See [`docs/DESIGN_SYSTEM.md`](docs/DESIGN_SYSTEM.md).
- Model-generated text is rendered only by `crates/ui-web/src/markdown.rs`.

## Decisions and documentation

- To change a recorded decision, create a new ADR in [`docs/adr/`](docs/adr/). Do not rewrite existing ADRs; add an "Update" section or create a new ADR.
- Update the documentation in `docs/` when a change alters behavior, and the requirement status in [`docs/PRODUCT.md`](docs/PRODUCT.md).

## Commits

- Short messages in the imperative mood, describing what the change does (for example: `Free llama-server memory after indexing`).
- One subject per commit whenever possible.

## Reporting bugs

Open an issue with:

- the NLMX and macOS versions, and the Mac model;
- the steps to reproduce, the expected behavior and the actual behavior;
- if relevant, excerpts from `~/Library/Application Support/dev.nlmx.desktop/logs/nlmx.jsonl`.

**Do not attach private documents.** If the problem depends on a specific PDF, try to reproduce it with a public file or one generated for the test.

## License

By contributing, you agree that your contributions will be licensed under the project's [MIT license](LICENSE).
