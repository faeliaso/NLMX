//! Regenerates the fixtures in `tests/fixtures/` (deterministic):
//! `cargo run -p nlmx-parser-office --example generate_fixtures`.

#[path = "../tests/common/mod.rs"]
mod common;

use std::{fs, path::PathBuf};

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    fs::create_dir_all(&dir).expect("create the fixtures directory");

    let contrato = common::contrato();
    let vendas = common::vendas();
    // Damaged: the end of the zip (its central directory) is missing.
    let damaged_docx = contrato[..contrato.len() * 2 / 5].to_vec();
    let damaged_xlsx = vendas[..vendas.len() * 2 / 5].to_vec();
    // Password-protected files are a compound file, not a zip.
    let mut protegido = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    protegido.extend_from_slice(&[0; 512]);

    for (name, bytes) in common::fixtures() {
        fs::write(dir.join(name), bytes).expect("write a fixture");
    }

    for (name, bytes) in [
        ("contrato.docx", contrato),
        ("corrompido.docx", damaged_docx),
        ("protegido.docx", protegido.clone()),
        ("vendas.xlsx", vendas),
        ("corrompido.xlsx", damaged_xlsx),
        ("protegido.xlsx", protegido),
    ] {
        fs::write(dir.join(name), bytes).expect("write a fixture");
    }
}
