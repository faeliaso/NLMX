# 0021 — CI and release flow

## Status
Accepted.

## Context
The project needs automatic validation of pull requests and a repeatable way to publish the DMG, with the least machinery that is still correct. The app requires macOS 27 and Apple Silicon.

## Decision
- Branches: `feature/*` → `develop` → `main`; releases are `vX.Y.Z` tags on `main`. No `release/*` branches.
- One CI workflow on GitHub's `xcode-27` runner (arm64, macOS 27; public preview at the time of writing): `make bootstrap`, `make lint`, `make test`, plus `make bundle` for PRs into `main`.
- One release workflow on tags `v*`: checks tag == version in `tauri.conf.json` and `Cargo.toml`, runs `make bundle`, publishes `dist/NLMX-<version>.dmg` with `gh release create`.
- Only `aarch64-apple-darwin`; no Intel, universal, Windows or Linux.
- Releases are unsigned and not notarized (ad-hoc); Apple credentials are not configured. Versioning is manual.

## Consequences
- If `xcode-27` changes or is retired, update the `runs-on` label in both workflows.
- Branch protection is configured by hand (see `docs/CI.md`).
- Signing/notarization, when added, goes into `release.yml` through the existing `make release` variables.
