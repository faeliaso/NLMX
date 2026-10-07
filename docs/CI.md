# Git flow, CI and releases

## Supported hardware

NLMX supports **only Macs with Apple Silicon (M1 or later) running macOS 27 or later**. The only build target is `aarch64-apple-darwin`. Intel/x86_64 and universal builds are not supported and the pipelines never build them.

Why: generation uses Apple Foundation Models through `/usr/bin/fm` (macOS 27), embeddings run on llama.cpp with Metal, and the app refuses Rosetta at runtime (see [ADR 0002](adr/0002-fm-cli-serve-over-uds.md) and [RELEASE.md](RELEASE.md)).

## Branches

```
feature/*  ──PR──▶  develop  ──PR──▶  main  ──tag vX.Y.Z──▶  GitHub Release
```

| Branch | Purpose |
|---|---|
| `feature/*` | Features, fixes, refactors, experiments. Branch from `develop`. |
| `develop` | Integration branch. |
| `main` | Stable code, always potentially publishable. Changes only through pull requests. |

There are no `release/*`, `hotfix/*` or `staging/*` branches. If one is needed in the future, decide it in a new ADR.

## Day to day

```sh
git checkout develop && git pull
git checkout -b feature/my-change
# work…
git add -A && git commit
git push -u origin feature/my-change
# open a PR: feature/my-change → develop
```

When `develop` is validated, open a PR `develop → main`.

## CI (`.github/workflows/ci.yml`)

Runs on pull requests into `develop`/`main` and on pushes to them, in one job on the `xcode-27` runner (arm64, macOS 27 — required by the app):

1. `make bootstrap` (Tailwind, HTMX, PDFium, llama.cpp; cached by the hash of `scripts/bootstrap.sh`);
2. `make lint` (`cargo fmt --check` + `cargo clippy -D warnings`);
3. `make test` (`cargo test --workspace`; no real model needed);
4. only for PRs into `main`: `make bundle` (unsigned DMG + `scripts/verify-bundle.sh`).

There is no Node/npm step: the project has no `package.json`; the CSS is built by `crates/ui-web/build.rs`.

## Releases (`.github/workflows/release.yml`)

Runs only when a tag `v*` is pushed:

1. checks that the tag matches the version in `apps/desktop/src-tauri/tauri.conf.json` and the workspace `Cargo.toml` (fails otherwise);
2. `make bundle` (DMG + verification);
3. creates the GitHub Release for the tag with `dist/NLMX-<version>.dmg` attached.

To publish:

```sh
# 1. on develop: bump the version in tauri.conf.json AND Cargo.toml [workspace.package]; merge into main by PR
git checkout main && git pull
git tag v0.4.0
git push origin v0.4.0      # triggers the release
```

Versioning is manual. There is no auto-update.

## Signing and notarization

Releases are currently **unsigned and not notarized** (ad-hoc signature). The release notes say so. On first launch macOS may need right-click › Open. Developer ID signing and notarization will be added later (see [RELEASE.md](RELEASE.md)); no Apple secrets are configured.

## Protecting `main` (manual, in GitHub)

Settings › Branches (or Rules › Rulesets) for `main`:

- require a pull request before merging (and approval, if there is more than one maintainer — with a single maintainer an approval requirement blocks the author unless bypass is allowed);
- require status checks: `ci`;
- block force pushes and branch deletion; no direct pushes.

Suggested for `develop`: require the `ci` check and block force pushes/deletion.
