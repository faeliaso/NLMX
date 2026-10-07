//! Compiles Tailwind CSS into `OUT_DIR/app.css` so the stylesheet is embedded in the binary.

use std::{env, path::PathBuf, process::Command};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../..");
    let ui = root.join("apps/desktop/ui");
    let tailwind = root.join("tools/bin/tailwindcss");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("app.css");

    for dir in ["pages", "components", "styles"] {
        println!("cargo::rerun-if-changed={}", ui.join(dir).display());
    }
    println!("cargo::rerun-if-changed={}", tailwind.display());

    if !tailwind.exists() {
        panic!(
            "Tailwind CLI not found at {}. Run `make bootstrap` first.",
            tailwind.display()
        );
    }

    let status = Command::new(&tailwind)
        .arg("--input")
        .arg(ui.join("styles/app.css"))
        .arg("--output")
        .arg(&out)
        .arg("--minify")
        .status()
        .expect("failed to run Tailwind CLI");
    assert!(status.success(), "Tailwind CLI failed with {status}");
}
