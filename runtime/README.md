# runtime/

Native runtime binaries bundled into the `.app` (never committed; `runtime/lib/` is gitignored).

Planned contents, fetched by a script once the corresponding adapter is implemented:

- `lib/libpdfium.dylib` — PDFium arm64 build (bblanchon/pdfium-binaries, pinned version), copied to
  `Contents/Frameworks` at bundle time and loaded via `@rpath`.

llama.cpp and sqlite-vec are linked statically and do not appear here. `fm` is provided by macOS.
