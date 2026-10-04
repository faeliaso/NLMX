//! Large and many files through the real indexing pipeline: time, throughput and the peak memory
//! of the process, plus what must be refused cleanly (over the limits, zip bombs) and what must
//! stay responsive (a big library behind the UI, imports while searching).
//!
//! `#[ignore]`d: it writes hundreds of megabytes and takes minutes (`make test-perf`). Sizes
//! scale with `NLMX_PERF_SCALE` (default 1; 0.1 for a quick look). Limits are generous: they
//! catch an order-of-magnitude regression, not noise; the numbers are printed for the report.

mod support;

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use nlmx_application::ports::{DocumentRepository, LexicalIndex};
use nlmx_domain::{
    ingestion::{DocumentStatus, ImportOutcome},
    retrieval::{LexicalQuery, RetrievalFilter},
};
use support::multiformat::{App, imported};

fn scale() -> f64 {
    std::env::var("NLMX_PERF_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0)
}

/// Resident memory of this process, in MiB.
fn rss_mb() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        / 1024
}

/// Runs `work` while sampling the process memory; returns its result, the time it took and the
/// peak memory above where it started.
async fn measured<T>(work: impl std::future::Future<Output = T>) -> (T, Duration, u64) {
    let start_rss = rss_mb();
    let peak = Arc::new(AtomicU64::new(start_rss));
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let (peak, stop) = (peak.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                peak.fetch_max(rss_mb(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(25));
            }
        })
    };
    let started = Instant::now();
    let result = work.await;
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    sampler.join().unwrap();
    peak.fetch_max(rss_mb(), Ordering::Relaxed);
    (
        result,
        elapsed,
        peak.load(Ordering::Relaxed).saturating_sub(start_rss),
    )
}

const WORDS: [&str; 24] = [
    "plataforma",
    "serviço",
    "relatório",
    "cliente",
    "contrato",
    "prazo",
    "cobertura",
    "análise",
    "processo",
    "sistema",
    "operação",
    "registro",
    "indicador",
    "resultado",
    "equipe",
    "projeto",
    "documento",
    "gestão",
    "qualidade",
    "métrica",
    "entrega",
    "fornecedor",
    "orçamento",
    "revisão",
];

/// Deterministic pseudo-text: `words` words from a small vocabulary.
fn sentence(seed: u64, words: usize) -> String {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = String::with_capacity(words * 9);
    for i in 0..words {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        if i > 0 {
            out.push(' ');
        }
        out.push_str(WORDS[(x % WORDS.len() as u64) as usize]);
    }
    out.push('.');
    out
}

fn write_markdown(path: &Path, megabytes: f64) -> usize {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let target = (megabytes * 1024.0 * 1024.0) as usize;
    let (mut written, mut section) = (0, 0u64);
    while written < target {
        section += 1;
        let mut block = format!("## Seção {section}\n\nMarcador mk{section}x.\n\n");
        for p in 0..8 {
            block.push_str(&sentence(section * 31 + p, 70));
            block.push_str("\n\n");
        }
        written += block.len();
        file.write_all(block.as_bytes()).unwrap();
    }
    section as usize
}

fn write_text(path: &Path, megabytes: f64) -> usize {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let target = (megabytes * 1024.0 * 1024.0) as usize;
    let (mut written, mut n) = (0, 0u64);
    // One very long paragraph (no blank line for ~200 KB), then ordinary ones.
    let long = (0..2500)
        .map(|i| sentence(i, 12))
        .collect::<Vec<_>>()
        .join(" ");
    file.write_all(long.as_bytes()).unwrap();
    file.write_all(b"\n\n").unwrap();
    written += long.len() + 2;
    while written < target {
        n += 1;
        let line = format!("{} Marcador tx{n}x.\n\n", sentence(n, 60));
        written += line.len();
        file.write_all(line.as_bytes()).unwrap();
    }
    n as usize
}

fn write_csv(path: &Path, rows: usize) {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    file.write_all(b"id;regiao;produto;unidades;valor;observacao\n")
        .unwrap();
    for i in 1..=rows {
        let marker = if i == rows / 2 { " marcadorcsv" } else { "" };
        writeln!(
            file,
            "{i};Região {};Produto {};{};{},{:02};{}{marker}",
            i % 7,
            i % 53,
            i * 3 % 1000,
            i % 900,
            i % 100,
            sentence(i as u64, 4)
        )
        .unwrap();
    }
}

/// An EPUB with `chapters` chapters of about `kb` KB each and one large stored image.
fn write_epub(path: &Path, chapters: usize, kb: usize, image_mb: usize) {
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};
    let mut zip = ZipWriter::new(std::fs::File::create(path).unwrap());
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();
    zip.start_file("META-INF/container.xml", deflated).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
    let mut manifest = String::new();
    let mut spine = String::new();
    let mut toc = String::new();
    for c in 1..=chapters {
        manifest.push_str(&format!(
            r#"<item id="c{c}" href="c{c}.xhtml" media-type="application/xhtml+xml"/>"#
        ));
        spine.push_str(&format!(r#"<itemref idref="c{c}"/>"#));
        toc.push_str(&format!(
            r#"<li><a href="c{c}.xhtml">Capítulo {c}</a></li>"#
        ));
    }
    manifest.push_str(r#"<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="img" href="capa.png" media-type="image/png"/>"#);
    zip.start_file("OEBPS/content.opf", deflated).unwrap();
    zip.write_all(format!(r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Livro grande</dc:title><dc:language>pt-BR</dc:language><dc:identifier id="id">x</dc:identifier></metadata><manifest>{manifest}</manifest><spine>{spine}</spine></package>"#).as_bytes()).unwrap();
    zip.start_file("OEBPS/nav.xhtml", deflated).unwrap();
    zip.write_all(format!(r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol>{toc}</ol></nav></body></html>"#).as_bytes()).unwrap();
    for c in 1..=chapters {
        zip.start_file(format!("OEBPS/c{c}.xhtml"), deflated)
            .unwrap();
        let mut body = format!(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Capítulo {c}</h1><p>Marcador ep{c}x.</p>"#
        );
        let mut i = 0;
        while body.len() < kb * 1024 {
            i += 1;
            body.push_str(&format!("<p>{}</p>", sentence((c * 1000 + i) as u64, 60)));
        }
        body.push_str("</body></html>");
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.start_file("OEBPS/capa.png", stored).unwrap();
    let chunk = vec![0x5Au8; 1024 * 1024];
    for _ in 0..image_mb {
        zip.write_all(&chunk).unwrap();
    }
    zip.finish().unwrap();
}

struct Report(Vec<String>);

impl Report {
    fn line(&mut self, text: String) {
        eprintln!("[perf] {text}");
        self.0.push(text);
    }
}

async fn index(app: &App, path: &Path) -> (ImportOutcome, Duration, u64) {
    measured(app.ingestion.import(path)).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "performance: writes hundreds of MB (make test-perf)"]
async fn large_files_are_indexed_with_bounded_memory() {
    let mut report = Report(Vec::new());
    let app = App::new("perf-large", true);
    let dir = app.dir.join("big");
    std::fs::create_dir_all(&dir).unwrap();
    let s = scale();

    // Markdown: ~20 MB, thousands of sections.
    let md = dir.join("grande.md");
    let sections = write_markdown(&md, 20.0 * s);
    let (outcome, took, peak) = index(&app, &md).await;
    let (id, chunks, status) = imported(outcome);
    report.line(format!(
        "markdown {:.0} MB, {sections} sections → {chunks} chunks in {:.1}s, peak +{peak} MB",
        20.0 * s,
        took.as_secs_f64()
    ));
    assert_eq!(status, DocumentStatus::Indexed);
    let late = LexicalIndex::search(
        app.db.as_ref(),
        &LexicalQuery::from_query(&format!("mk{}x", sections - 1)),
        5,
        &RetrievalFilter::default(),
    )
    .await
    .unwrap();
    assert!(
        late.iter().any(|h| h.document_id == id),
        "a section at the end is searchable"
    );
    assert!(
        took < Duration::from_secs((120.0 * s.max(0.25)) as u64),
        "{took:?}"
    );
    assert!(peak < 2500, "peak memory +{peak} MB");

    // TXT: ~50 MB (the limit is 64 MiB), one 200 KB paragraph without blank lines.
    let txt = dir.join("grande.txt");
    let paragraphs = write_text(&txt, 50.0 * s);
    let (outcome, took, peak) = index(&app, &txt).await;
    let (id, chunks, status) = imported(outcome);
    report.line(format!(
        "txt {:.0} MB, {paragraphs} paragraphs → {chunks} chunks in {:.1}s, peak +{peak} MB",
        50.0 * s,
        took.as_secs_f64()
    ));
    assert_eq!(status, DocumentStatus::Indexed);
    let late = LexicalIndex::search(
        app.db.as_ref(),
        &LexicalQuery::from_query(&format!("tx{}x", paragraphs - 1)),
        5,
        &RetrievalFilter::default(),
    )
    .await
    .unwrap();
    assert!(late.iter().any(|h| h.document_id == id));
    assert!(
        took < Duration::from_secs((300.0 * s.max(0.25)) as u64),
        "{took:?}"
    );
    assert!(peak < 3500, "peak memory +{peak} MB");

    // CSV: just under the 250 000-row limit.
    let rows = (240_000.0 * s) as usize;
    let csv = dir.join("grande.csv");
    write_csv(&csv, rows.max(1000));
    let (outcome, took, peak) = index(&app, &csv).await;
    let (id, chunks, status) = imported(outcome);
    report.line(format!(
        "csv {rows} rows → {chunks} chunks in {:.1}s, peak +{peak} MB",
        took.as_secs_f64()
    ));
    assert_eq!(status, DocumentStatus::Indexed);
    let hit = LexicalIndex::search(
        app.db.as_ref(),
        &LexicalQuery::from_query("marcadorcsv"),
        5,
        &RetrievalFilter::default(),
    )
    .await
    .unwrap();
    assert!(
        hit.iter().any(|h| h.document_id == id),
        "the row in the middle is searchable"
    );
    assert!(
        took < Duration::from_secs((300.0 * s.max(0.25)) as u64),
        "{took:?}"
    );
    assert!(peak < 3500, "peak memory +{peak} MB");

    // EPUB: hundreds of chapters and a 30 MB cover that must not be read as text.
    let epub = dir.join("grande.epub");
    let chapters = ((400.0 * s) as usize).max(20);
    write_epub(&epub, chapters, 24, (30.0 * s).max(1.0) as usize);
    let (outcome, took, peak) = index(&app, &epub).await;
    let (id, chunks, status) = imported(outcome);
    report.line(format!(
        "epub {chapters} chapters + image → {chunks} chunks in {:.1}s, peak +{peak} MB",
        took.as_secs_f64()
    ));
    assert_eq!(status, DocumentStatus::Indexed);
    let hit = LexicalIndex::search(
        app.db.as_ref(),
        &LexicalQuery::from_query(&format!("ep{chapters}x")),
        5,
        &RetrievalFilter::default(),
    )
    .await
    .unwrap();
    assert!(
        hit.iter().any(|h| h.document_id == id),
        "the last chapter is searchable"
    );
    assert!(
        took < Duration::from_secs((180.0 * s.max(0.25)) as u64),
        "{took:?}"
    );
    assert!(peak < 2500, "peak memory +{peak} MB");

    // Nothing is duplicated by indexing them all, and reindexing a big one is stable.
    let before = app.index_sizes();
    let (_, took, peak) = measured(app.ingestion.reindex(id)).await;
    report.line(format!(
        "reindex of the epub in {:.1}s, peak +{peak} MB",
        took.as_secs_f64()
    ));
    assert_eq!(app.index_sizes(), before);
    eprintln!("{}", report.0.join("\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "performance (make test-perf)"]
async fn files_over_the_limits_and_zip_bombs_are_refused_without_exhausting_memory() {
    let app = App::new("perf-limits", true);
    let dir = app.dir.join("limits");
    std::fs::create_dir_all(&dir).unwrap();

    // A TXT over the limit (64 MiB) and a CSV over the row limit (250 000).
    let big_txt = dir.join("enorme.txt");
    write_text(&big_txt, 70.0);
    let big_csv = dir.join("enorme.csv");
    write_csv(&big_csv, 260_000);
    for path in [&big_txt, &big_csv] {
        let (outcome, took, peak) = index(&app, path).await;
        let ImportOutcome::Failed {
            id: Some(id),
            reason,
        } = outcome
        else {
            panic!("{outcome:?}")
        };
        eprintln!(
            "[perf] {} refused in {:.1}s, peak +{peak} MB: {reason}",
            path.display(),
            took.as_secs_f64()
        );
        assert_eq!(app.status(id).await, DocumentStatus::Failed);
        assert!(!reason.is_empty());
        assert!(took < Duration::from_secs(60), "{took:?}");
        assert!(
            peak < 1500,
            "the refusal itself must not need the file in memory: +{peak} MB"
        );
        assert_eq!(
            app.count(&format!(
                "SELECT count(*) FROM document_chunks WHERE document_id = {id}"
            )),
            0
        );
    }

    // A zip bomb: a chapter that is ~1 GB of text once inflated.
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};
    let bomb = dir.join("bomba.epub");
    {
        let mut zip = ZipWriter::new(std::fs::File::create(&bomb).unwrap());
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", deflated).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();
        zip.start_file("META-INF/container.xml", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="c.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
        zip.start_file("c.opf", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>B</dc:title></metadata><manifest><item id="a" href="a.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/></spine></package>"#).unwrap();
        zip.start_file("a.xhtml", deflated).unwrap();
        zip.write_all(b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>")
            .unwrap();
        let chunk = vec![b'a'; 1024 * 1024];
        for _ in 0..1024 {
            zip.write_all(&chunk).unwrap();
        }
        zip.write_all(b"</p></body></html>").unwrap();
        zip.finish().unwrap();
    }
    let on_disk = std::fs::metadata(&bomb).unwrap().len();
    let (outcome, took, peak) = index(&app, &bomb).await;
    eprintln!(
        "[perf] zip bomb ({on_disk} bytes on disk) → {outcome:?} in {:.1}s, peak +{peak} MB",
        took.as_secs_f64()
    );
    assert!(
        matches!(outcome, ImportOutcome::Failed { .. }),
        "{outcome:?}"
    );
    assert!(peak < 1500, "+{peak} MB");

    // Deeply nested XHTML and a CSV record of 40 MB in one cell: refused or handled, not a crash.
    let nested = dir.join("aninhado.epub");
    {
        let mut zip = ZipWriter::new(std::fs::File::create(&nested).unwrap());
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", deflated).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();
        zip.start_file("META-INF/container.xml", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="c.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
        zip.start_file("c.opf", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>N</dc:title></metadata><manifest><item id="a" href="a.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/></spine></package>"#).unwrap();
        zip.start_file("a.xhtml", deflated).unwrap();
        let depth = 20_000;
        let mut xml = String::from("<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>");
        xml.push_str(&"<div>".repeat(depth));
        xml.push_str("texto no fundo");
        xml.push_str(&"</div>".repeat(depth));
        xml.push_str("</body></html>");
        zip.write_all(xml.as_bytes()).unwrap();
        zip.finish().unwrap();
    }
    let (outcome, took, _) = index(&app, &nested).await;
    eprintln!(
        "[perf] nested xhtml → {outcome:?} in {:.1}s",
        took.as_secs_f64()
    );
    assert!(took < Duration::from_secs(60));

    let wide = dir.join("celula.csv");
    {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&wide).unwrap());
        f.write_all(b"id;texto\n1;\"").unwrap();
        let chunk = vec![b'x'; 1024 * 1024];
        for _ in 0..40 {
            f.write_all(&chunk).unwrap();
        }
        f.write_all(b"\"\n2;fim\n").unwrap();
    }
    let (outcome, took, peak) = index(&app, &wide).await;
    eprintln!(
        "[perf] 40 MB cell → {outcome:?} in {:.1}s, peak +{peak} MB",
        took.as_secs_f64()
    );
    assert!(took < Duration::from_secs(120));
    assert!(peak < 2000, "+{peak} MB");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "performance (make test-perf)"]
async fn many_files_and_concurrent_imports_and_searches() {
    let app = App::new("perf-many", true);
    let dir = app.dir.join("many");
    std::fs::create_dir_all(&dir).unwrap();
    let count = ((200.0 * scale()) as usize).max(20);
    let mut paths = Vec::new();
    for i in 0..count {
        let (name, body) = match i % 4 {
            0 => (
                format!("n{i}.md"),
                format!(
                    "# Nota {i}\n\nMarcador mn{i}x.\n\n{}\n",
                    sentence(i as u64, 200)
                ),
            ),
            1 => (
                format!("n{i}.txt"),
                format!("Marcador tn{i}x.\n\n{}\n", sentence(i as u64, 200)),
            ),
            2 => (
                format!("n{i}.csv"),
                format!("id;texto\n{i};marcador cn{i}x {}\n", sentence(i as u64, 10)),
            ),
            _ => (
                format!("n{i}.md"),
                format!(
                    "## Outra {i}\n\nMarcador mn{i}x.\n\n{}\n",
                    sentence(i as u64 + 7, 300)
                ),
            ),
        };
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        paths.push(path);
    }
    let ingestion = Arc::new(app.ingestion);
    let db = app.db.clone();

    // Four importers register at the same time while one worker processes what they register,
    // and searches run throughout (what the queue and the chat do in the app).
    let (queue_tx, mut queue_rx) = tokio::sync::mpsc::unbounded_channel();
    let started = Instant::now();
    let (_, took, peak) = measured(async {
        let mut registrars = Vec::new();
        for quarter in paths.chunks(count.div_ceil(4)).map(<[PathBuf]>::to_vec) {
            let (ingestion, tx) = (ingestion.clone(), queue_tx.clone());
            registrars.push(tokio::spawn(async move {
                for path in quarter {
                    let enqueued = ingestion.enqueue(&path).await;
                    tx.send(enqueued).unwrap();
                }
            }));
        }
        drop(queue_tx);
        let worker = {
            let ingestion = ingestion.clone();
            tokio::spawn(async move {
                let mut outcomes = Vec::new();
                while let Some(enqueued) = queue_rx.recv().await {
                    outcomes.push(ingestion.process(enqueued).await);
                }
                outcomes
            })
        };
        let searcher = {
            let db = db.clone();
            tokio::spawn(async move {
                let mut searches = 0;
                let mut worst = Duration::ZERO;
                while searches < 400 {
                    let t = Instant::now();
                    let _ = LexicalIndex::search(
                        db.as_ref(),
                        &LexicalQuery::from_query("marcador"),
                        10,
                        &RetrievalFilter::default(),
                    )
                    .await
                    .expect("a search never fails while documents are being indexed");
                    worst = worst.max(t.elapsed());
                    searches += 1;
                    tokio::task::yield_now().await;
                }
                worst
            })
        };
        for r in registrars {
            r.await.unwrap();
        }
        let outcomes = worker.await.unwrap();
        let worst_search = searcher.await.unwrap();
        eprintln!("[perf] worst search while indexing: {worst_search:?}");
        outcomes
    })
    .await
    .0
    .into_iter()
    .fold(((), Duration::ZERO, 0), |acc, o| {
        assert!(
            matches!(
                o,
                ImportOutcome::Imported {
                    status: DocumentStatus::Indexed,
                    ..
                }
            ),
            "{o:?}"
        );
        acc
    });
    let _ = (took, peak);
    let total = started.elapsed();
    eprintln!(
        "[perf] {count} files indexed concurrently in {:.1}s ({:.0} files/s)",
        total.as_secs_f64(),
        count as f64 / total.as_secs_f64()
    );

    let documents = db.list().await.unwrap();
    assert_eq!(documents.len(), count, "every file is exactly one document");
    assert!(
        documents
            .iter()
            .all(|d| d.status == DocumentStatus::Indexed && d.chunk_count > 0)
    );
    let chunks: i64 = app_count(&app.dir, "SELECT count(*) FROM document_chunks");
    let vectors: i64 = app_count(&app.dir, "SELECT count(*) FROM chunk_embeddings");
    assert_eq!(chunks, vectors, "one vector per chunk, none duplicated");
    // The same files again are duplicates, not new documents or chunks.
    for path in paths.iter().take(10) {
        assert!(matches!(
            ingestion.import(path).await,
            ImportOutcome::Duplicate { .. }
        ));
    }
    assert_eq!(
        app_count(&app.dir, "SELECT count(*) FROM document_chunks"),
        chunks
    );
}

fn app_count(dir: &Path, sql: &str) -> i64 {
    rusqlite::Connection::open(dir.join("nlmx.sqlite3"))
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

/// What the screens cost with a big library behind them: 1 000 documents of every format and
/// 30 000 chunks, as the router the app serves renders them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "performance (make test-perf)"]
async fn the_screens_stay_responsive_with_a_big_library() {
    use axum::body::Body;
    use http::Request;
    use http_body_util::BodyExt;
    use nlmx_application::{
        services::{
            free_chat::FreeChat,
            rag::{RagEngine, RagOptions},
            retrieval::HybridRetriever,
            retriever::Retriever,
        },
        use_cases::{ChatService, GetSystemStatus, Indexing, IndexingActivity, ViewDocument},
    };
    use nlmx_testing::{FakeLlmProvider, FakeRuntime};
    use nlmx_ui_web::{AppState, router};
    use tower::ServiceExt;

    let app = App::new("perf-ui", true);
    let documents = ((1000.0 * scale()) as usize).max(50);
    {
        let mut conn = rusqlite::Connection::open(app.dir.join("nlmx.sqlite3")).unwrap();
        let tx = conn.transaction().unwrap();
        for i in 1..=documents {
            let (format, ext) = ["pdf", "markdown", "text", "csv", "epub"]
                .iter()
                .zip(["pdf", "md", "txt", "csv", "epub"])
                .nth(i % 5)
                .map(|(f, e)| (*f, e))
                .unwrap();
            tx.execute(
                "INSERT INTO documents (id, sha256, title, original_filename, library_path, file_size, status, format, imported_at)
                 VALUES (?1, printf('%064x', ?1), ?2, ?3, ?4, 1000, 'indexed', ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now', printf('-%d seconds', ?1)))",
                rusqlite::params![i as i64, format!("Documento {i}"), format!("arquivo-{i}.{ext}"), format!("/lib/{i}.{ext}"), format],
            )
            .unwrap();
            for ordinal in 0..30 {
                tx.execute(
                    "INSERT INTO document_chunks (document_id, ordinal, text, token_count, page_start, page_end, content_hash)
                     VALUES (?1, ?2, ?3, 50, 1, 1, ?4)",
                    rusqlite::params![i as i64, ordinal as i64, sentence((i * 100 + ordinal) as u64, 40), format!("h{i}-{ordinal}")],
                )
                .unwrap();
            }
        }
        tx.commit().unwrap();
    }

    let llm = Arc::new(FakeLlmProvider::available().answering("ok"));
    let retriever = Arc::new(Retriever::new(Arc::new(HybridRetriever::new(
        app.model.clone(),
        app.db.clone(),
        app.db.clone(),
        app.db.clone(),
    ))));
    let chat = Arc::new(ChatService {
        conversations: app.db.clone(),
        rag: Arc::new(RagEngine::new(retriever, app.db.clone(), llm.clone())),
        free: Arc::new(FreeChat::new(llm.clone())),
        options: RagOptions::default(),
    });
    let support::multiformat::App {
        db,
        ingestion,
        embedder,
        model,
        ..
    } = app;
    let ingestion = Arc::new(ingestion);
    let indexing = Arc::new(Indexing {
        reader: db.clone(),
        documents: db.clone(),
        embeddings: model,
        embedder,
        ingestion: Ok(ingestion.clone()),
        activity: Arc::new(IndexingActivity::default()),
    });
    let router = router(AppState {
        system_status: Arc::new(GetSystemStatus::new(
            llm,
            db.clone(),
            Ok("x".into()),
            Arc::new(FakeRuntime::llama()),
        )),
        ingestion: Ok(ingestion),
        chat: Ok(chat),
        viewer: Ok(Arc::new(ViewDocument::new(
            support::shared_engine(),
            db.clone(),
        ))),
        remover: Err("n/a".into()),
        diagnostics: None,
        models: None,
        indexing: Ok(indexing),
    });

    for (name, path, limit_ms) in [
        ("Documentos", "/fragments/documents", 500),
        ("Indexação", "/indexing", 500),
        ("Chat (1 000 documentos no seletor)", "/chat", 1000),
        ("Informações da fonte", "/sources/501", 200),
    ] {
        let mut worst = Duration::ZERO;
        let mut bytes = 0;
        for _ in 0..5 {
            let started = Instant::now();
            let response = router
                .clone()
                .oneshot(
                    Request::get(path)
                        .header("HX-Request", "true")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(
                response.status().is_success(),
                "{path}: {}",
                response.status()
            );
            bytes = response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .len();
            worst = worst.max(started.elapsed());
        }
        eprintln!(
            "[perf] {name}: {documents} documents → worst of 5 {:.0} ms, {} KB",
            worst.as_secs_f64() * 1000.0,
            bytes / 1024
        );
        assert!(
            worst < Duration::from_millis(limit_ms),
            "{name} took {worst:?}"
        );
    }
}
