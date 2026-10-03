# models/

Local GGUF embedding models for development and tests (`*.gguf` is gitignored).

End users never use this directory: the app downloads models on demand into
`~/Library/Application Support/dev.nlmx.desktop/models/`. The embedded model catalog will live
in `crates/adapters/models-catalog`.
