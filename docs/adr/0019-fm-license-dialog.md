# ADR 0019 — "Ative a IA local": license dialog driven by the real `fm` state

Status: accepted

## Context

Apple Foundation Models through `/usr/bin/fm` needs a one-time, machine-wide license accepted with
`sudo fm license`. The app showed a technical badge/alert only, and its "check again" could return
a cached answer (30 s `status_ttl`).

## Decision

- The app never runs `sudo fm license`, never asks for or stores a password and never scripts the
  Terminal. The user runs the command; the app only re-reads the state (`architecture.rs`
  `nothing_runs_the_license_command_or_asks_for_a_password` guards this).
- One detection path: `fm available` (exit 69 ⇒ `LicenseRequired`). `fm license --status` is not
  used.
- `LanguageModelStatus::NotInstalled` is separate from `Incompatible` (`compat::Incompatibility`):
  a missing `fm` must not suggest `sudo fm license`.
- `LlmProvider::recheck()` (default = `status()`) bypasses the cache; `FoundationModelsProvider`
  serializes probes so simultaneous re-checks share one `fm available` run. Probe failures show a
  generic message in the UI; details go only to the logs.
- The dialog (`ui-web/src/fm_setup.rs`, `components/fm_setup*.html`) is fetched at every full page
  load (`GET /fragments/fm-setup`) and is empty when authorized. No `firstLaunch`/`hasSeenModal`
  flag: authorization lost later brings it back. "Verificar novamente" is
  `POST /fragments/fm-setup/recheck`; on success the dialog closes by itself, without restarting.
- Copying uses the existing `data-copy-from` (client-side clipboard, no command, no request).

## Consequences

The dialog can be closed ("Agora não"); the Chat alert and the footer badge keep reporting the
state. There is no automated JS test (no JS tooling in the project); behaviour is covered by router
tests with `FakeLlmProvider` and by `llm-fm` tests with a fake `fm`.
