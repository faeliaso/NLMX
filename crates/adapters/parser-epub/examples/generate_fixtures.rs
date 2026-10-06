//! Regenerates the fixtures in `tests/fixtures/` (deterministic):
//! `cargo run -p nlmx-parser-epub --example generate_fixtures`.

#[path = "../tests/common/mod.rs"]
mod common;

use std::{fs, path::PathBuf};

use common::{Book, Chapter, livro};

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    fs::create_dir_all(&dir).expect("create the fixtures directory");

    let good = livro().build();

    // DRM: the content is encrypted with a non-font algorithm.
    let mut drm = livro();
    drm.encryption = vec!["http://www.w3.org/2001/04/xmlenc#aes128-cbc".into()];

    // No text at all: one chapter with only an image.
    let empty = Book::new(vec![Chapter::new(
        "ch1.xhtml",
        "<div><img src=\"capa.png\" alt=\"\"/></div>",
    )]);

    // Damaged: the end of the zip (its central directory) is missing.
    let damaged = good[..good.len() * 2 / 5].to_vec();

    for (name, bytes) in [
        ("livro.epub", good),
        ("drm.epub", drm.build()),
        ("sem-texto.epub", empty.build()),
        ("corrompido.epub", damaged),
    ] {
        fs::write(dir.join(name), bytes).expect("write a fixture");
    }
}
