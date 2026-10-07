//! `nlmx-desktop --self-check`: proves an installed app runs on its own — PDFium, SQLite +
//! sqlite-vec and the llama.cpp sidecar load from the bundle — without a window and without
//! touching the user's data (everything happens in a temporary directory). Prints JSON; the exit
//! code is non-zero when a bundled component fails. Used by `scripts/verify-bundle.sh`.

use std::path::{Path, PathBuf};

use nlmx_application::ports::{DocumentEngine, InferenceRuntime, LlmProvider, StorageDiagnostics};
use serde_json::{Value, json};

/// A one-page PDF with a line of text (Helvetica), written by hand.
fn sample_pdf() -> Vec<u8> {
    let content = "BT /F1 12 Tf 72 720 Td (NLMX self-check) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

fn bundle_dir() -> Option<PathBuf> {
    // Contents/MacOS/nlmx-desktop → Contents
    std::env::current_exe()
        .ok()?
        .parent()?
        .parent()
        .map(Path::to_path_buf)
}

async fn check(dir: &Path) -> (Value, bool) {
    let mut ok = true;

    // PDFium: load, open a PDF, extract its text.
    let pdfium = match nlmx_pdf_pdfium::PdfiumDocumentEngine::from_default_location() {
        Err(e) => {
            ok = false;
            json!({ "ok": false, "error": e.to_string() })
        }
        Ok(engine) => {
            let file = dir.join("self-check.pdf");
            let result = async {
                std::fs::write(&file, sample_pdf()).map_err(|e| e.to_string())?;
                let doc = engine.open(&file).await.map_err(|e| e.to_string())?;
                let text = engine
                    .extract_text(doc, 1)
                    .await
                    .map_err(|e| e.to_string())?;
                let _ = engine.close(doc).await;
                Ok::<_, String>(text)
            }
            .await;
            match result {
                Ok(text) if text.contains("self-check") => {
                    json!({ "ok": true, "build": nlmx_pdf_pdfium::PDFIUM_BUILD })
                }
                other => {
                    ok = false;
                    json!({ "ok": false, "error": format!("{other:?}") })
                }
            }
        }
    };

    // SQLite + sqlite-vec (statically linked) and the migrations.
    let database = match nlmx_store_sqlite::Database::open(dir.join("self-check.sqlite3")) {
        Err(e) => {
            ok = false;
            json!({ "ok": false, "error": e.message })
        }
        Ok(db) => match db.info().await {
            Ok(info) => json!({
                "ok": info.schema_version == info.latest_schema_version,
                "sqlite": info.sqlite_version,
                "sqlite_vec": info.vector_extension_version,
                "schema": info.schema_version,
            }),
            Err(e) => {
                ok = false;
                json!({ "ok": false, "error": e.message })
            }
        },
    };
    ok &= database["ok"].as_bool().unwrap_or(false);

    // llama.cpp sidecar (`--version` loads every library it links from Contents/Frameworks).
    let llama = match nlmx_embed_llama::LlamaCppRuntime::detect().info().await {
        Ok(info) => json!({
            "ok": true,
            "build": info.build,
            "bundled": info.bundled,
            "path": info.path,
        }),
        Err(e) => {
            ok = false;
            json!({ "ok": false, "error": e })
        }
    };

    // Apple Foundation Models is part of macOS, not of the bundle: reported, not required.
    let fm = nlmx_llm_fm::FoundationModelsProvider::system(dir.join("run"));
    let fm_status = format!("{:?}", fm.status().await);

    let contents = bundle_dir();
    let licenses = contents
        .as_ref()
        .map(|c| c.join("Resources/licenses"))
        .filter(|p| p.is_dir())
        .map(|p| {
            let mut names: Vec<String> = std::fs::read_dir(p)
                .map(|e| {
                    e.flatten()
                        .map(|f| f.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        });
    let report = json!({
        "ok": ok,
        "version": env!("CARGO_PKG_VERSION"),
        "arch": std::env::consts::ARCH,
        "debug_build": cfg!(debug_assertions),
        "pdfium": pdfium,
        "database": database,
        "llama_cpp": llama,
        "apple_foundation_models": fm_status,
        "licenses": licenses,
        "data_dir": dirs_data_dir().map(|p| p.display().to_string()),
    });
    (report, ok)
}

/// Where the app keeps its data (not touched by the self-check).
fn dirs_data_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/dev.nlmx.desktop"))
}

/// Runs the checks and returns the process exit code.
pub fn run() -> i32 {
    let dir = std::env::temp_dir().join(format!("nlmx-self-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create {}: {e}", dir.display());
        return 2;
    }
    let (report, ok) = tauri::async_runtime::block_on(check(&dir));
    let _ = std::fs::remove_dir_all(&dir);
    println!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    if ok { 0 } else { 1 }
}
