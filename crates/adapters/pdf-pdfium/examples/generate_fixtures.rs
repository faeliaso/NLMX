//! Writes the small test PDFs in `tests/fixtures/` by hand, so their text, fonts, coordinates and
//! metadata are known exactly. Run after changing it:
//!
//!     cargo run -p nlmx-pdf-pdfium --example generate_fixtures
//!
//! Page size is US Letter (612 × 792 pt). Fonts are the standard Helvetica faces with WinAnsiEncoding.

use std::{fs, path::Path};

const PAGE: (u32, u32) = (612, 792);

/// Minimal PDF writer: numbered objects, one xref table, no compression.
struct Pdf {
    objects: Vec<Option<Vec<u8>>>,
}

impl Pdf {
    fn new() -> Self {
        Self {
            objects: Vec::new(),
        }
    }

    /// Reserves an object number to be filled later (needed for parent/child references).
    fn reserve(&mut self) -> usize {
        self.objects.push(None);
        self.objects.len()
    }

    fn set(&mut self, id: usize, body: impl Into<Vec<u8>>) {
        self.objects[id - 1] = Some(body.into());
    }

    fn add(&mut self, body: impl Into<Vec<u8>>) -> usize {
        let id = self.reserve();
        self.set(id, body);
        id
    }

    fn stream(&mut self, dict: &str, data: &[u8]) -> usize {
        let mut body = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.add(body)
    }

    fn finish(self, root: usize, info: Option<usize>) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(body.as_ref().expect("every reserved object is set"));
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
        );
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        let info = info
            .map(|id| format!(" /Info {id} 0 R"))
            .unwrap_or_default();
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root {root} 0 R{info} >>\nstartxref\n{xref}\n%%EOF\n",
                self.objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }
}

/// PDF string literal in WinAnsiEncoding (Latin-1 plus a few CP1252 punctuation marks).
fn text(s: &str) -> Vec<u8> {
    let mut out = vec![b'('];
    for c in s.chars() {
        let byte = match c {
            '—' => 0x97,
            '•' => 0x95,
            '–' => 0x96,
            '“' => 0x93,
            '”' => 0x94,
            c if (c as u32) <= 0xFF => c as u32 as u8,
            other => panic!("{other:?} is not representable in WinAnsiEncoding"),
        };
        if matches!(byte, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(byte);
    }
    out.push(b')');
    out
}

/// `BT /font size Tf x y Td (text) Tj ET` — (x, y) is the baseline origin, bottom-left based.
fn line(font: &str, size: u32, x: u32, y: u32, s: &str) -> Vec<u8> {
    let mut out = format!("BT /{font} {size} Tf {x} {y} Td ").into_bytes();
    out.extend(text(s));
    out.extend_from_slice(b" Tj ET\n");
    out
}

/// Draws image `/Im1` at (x, y) bottom-left with the given size in points.
fn image(x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
    format!("q {w} 0 0 {h} {x} {y} cm /Im1 Do Q\n").into_bytes()
}

enum Content {
    Text(Vec<u8>),
    Image(Vec<u8>),
    Blank,
    /// Text on a page with `/Rotate` (degrees).
    Rotated(u32, Vec<u8>),
}

/// A document whose pages share Helvetica (F1), Helvetica-Bold (F2) and one 4×2 RGB image (Im1).
fn document(pages: Vec<Content>, info: Option<Vec<u8>>) -> Vec<u8> {
    let mut pdf = Pdf::new();
    let catalog = pdf.reserve();
    let pages_id = pdf.reserve();
    let regular = pdf
        .add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>");
    let bold = pdf.add(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>",
    );
    // 4×2 pixels: red, green, blue, white / black, gray, yellow, cyan.
    let pixels: [u8; 24] = [
        255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, //
        0, 0, 0, 128, 128, 128, 255, 255, 0, 0, 255, 255,
    ];
    let img = pdf.stream(
        "/Type /XObject /Subtype /Image /Width 4 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8",
        &pixels,
    );
    let resources =
        format!("<< /Font << /F1 {regular} 0 R /F2 {bold} 0 R >> /XObject << /Im1 {img} 0 R >> >>");

    let mut kids = Vec::new();
    for page in pages {
        let (data, rotate) = match page {
            Content::Text(data) | Content::Image(data) => (data, None),
            Content::Blank => (Vec::new(), None),
            Content::Rotated(degrees, data) => (data, Some(degrees)),
        };
        let contents = pdf.stream("", &data);
        let rotate = rotate.map(|d| format!(" /Rotate {d}")).unwrap_or_default();
        let id = pdf.add(format!(
            "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {} {}]{rotate} /Resources {resources} /Contents {contents} 0 R >>",
            PAGE.0, PAGE.1
        ));
        kids.push(format!("{id} 0 R"));
    }
    pdf.set(
        pages_id,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            kids.len()
        ),
    );
    pdf.set(
        catalog,
        format!("<< /Type /Catalog /Pages {pages_id} 0 R >>"),
    );
    let info = info.map(|dict| pdf.add(dict));
    pdf.finish(catalog, info)
}

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    fs::create_dir_all(&dir).unwrap();

    // text.pdf — two text pages with metadata.
    let mut page1 = line("F2", 18, 72, 720, "Contrato de Exemplo");
    page1.extend(line(
        "F1",
        11,
        72,
        690,
        "Cláusula 1 — Carência de 180 dias.",
    ));
    page1.extend(line(
        "F1",
        11,
        72,
        674,
        "Cobertura ambulatorial e hospitalar.",
    ));
    let page2 = line("F1", 11, 72, 720, "Página dois");
    let mut info = b"<< /Title ".to_vec();
    info.extend(text("Contrato de Exemplo"));
    info.extend_from_slice(b" /Author ");
    info.extend(text("Equipe NLMX"));
    info.extend_from_slice(b" /Subject ");
    info.extend(text("Fixture de teste"));
    info.extend_from_slice(
        b" /Keywords (rag, teste) /Creator (generate_fixtures) /Producer (NLMX) \
          /CreationDate (D:20260102030405Z) /ModDate (D:20260203040506Z) >>",
    );
    write(
        &dir,
        "text.pdf",
        document(vec![Content::Text(page1), Content::Text(page2)], Some(info)),
    );

    // scanned.pdf — one image-only page (no text layer), image at x=100 y=500, 200×100 pt.
    write(
        &dir,
        "scanned.pdf",
        document(vec![Content::Image(image(100, 500, 200, 100))], None),
    );

    // mixed.pdf — text page, image-only page, blank page.
    write(
        &dir,
        "mixed.pdf",
        document(
            vec![
                Content::Text(line("F1", 12, 72, 720, "Introdução")),
                Content::Image(image(100, 500, 200, 100)),
                Content::Blank,
            ],
            None,
        ),
    );

    // report.pdf — 3 pages for the ingestion pipeline: repeated header and "Página N de 3" footer,
    // bold 14 pt headings, a word hyphenated across lines, a bullet list, and a paragraph that
    // continues from page 2 onto page 3.
    let chrome = |n: u32| {
        let mut out = line("F1", 9, 72, 760, "NLMX — Relatório");
        out.extend(line("F1", 9, 270, 30, &format!("Página {n} de 3")));
        out
    };
    let mut p1 = chrome(1);
    p1.extend(line("F2", 14, 72, 700, "1. Introdução"));
    p1.extend(line(
        "F1",
        11,
        72,
        680,
        "Este relatório descreve o período de carên-",
    ));
    p1.extend(line(
        "F1",
        11,
        72,
        666,
        "cia aplicado aos contratos e as regras de",
    ));
    p1.extend(line("F1", 11, 72, 652, "cobertura vigentes."));
    p1.extend(line("F2", 14, 72, 620, "2. Coberturas"));
    p1.extend(line("F1", 11, 72, 600, "• Consultas ambulatoriais"));
    p1.extend(line("F1", 11, 72, 586, "• Exames laboratoriais"));
    p1.extend(line("F1", 11, 72, 572, "• Internação hospitalar"));
    let mut p2 = chrome(2);
    p2.extend(line("F2", 14, 72, 700, "3. Prazos"));
    p2.extend(line(
        "F1",
        11,
        72,
        680,
        "Os prazos contam a partir da assinatura e",
    ));
    p2.extend(line("F1", 11, 72, 666, "o prazo de carência termina após"));
    let mut p3 = chrome(3);
    p3.extend(line("F1", 11, 72, 700, "cento e oitenta dias corridos."));
    p3.extend(line("F2", 14, 72, 660, "4. Disposições finais"));
    p3.extend(line(
        "F1",
        11,
        72,
        640,
        "Casos omissos serão resolvidos pela operadora.",
    ));
    let mut report_info = b"<< /Title ".to_vec();
    report_info.extend(text("Relatório de Coberturas"));
    report_info.extend_from_slice(b" /Author (NLMX) >>");
    write(
        &dir,
        "report.pdf",
        document(
            vec![
                Content::Text(p1.clone()),
                Content::Text(p2.clone()),
                Content::Text(p3.clone()),
            ],
            Some(report_info),
        ),
    );

    // report-copy.pdf — the same pages with different metadata (so a different file hash):
    // imported as a second document whose chunks duplicate report.pdf's.
    let mut copy_info = b"<< /Title ".to_vec();
    copy_info.extend(text("Relatório de Coberturas (cópia)"));
    copy_info.extend_from_slice(b" /Author (Outra equipe) >>");
    write(
        &dir,
        "report-copy.pdf",
        document(
            vec![Content::Text(p1), Content::Text(p2), Content::Text(p3)],
            Some(copy_info),
        ),
    );

    // rotated.pdf — page 1 upright, page 2 rotated 90°: text and boxes must stay in the page.
    write(
        &dir,
        "rotated.pdf",
        document(
            vec![
                Content::Text(line("F1", 12, 72, 720, "Página normal")),
                Content::Rotated(90, line("F1", 12, 72, 720, "Página girada")),
            ],
            None,
        ),
    );

    // unicode.pdf — Portuguese and other Latin-1 accents in regular and bold fonts.
    let mut accents = line(
        "F1",
        12,
        72,
        720,
        "Ação, coração, pão, avó, órgão, júri, câmbio",
    );
    accents.extend(line(
        "F2",
        12,
        72,
        700,
        "Über, niño, façade, crème brûlée — “aspas”",
    ));
    write(
        &dir,
        "unicode.pdf",
        document(vec![Content::Text(accents)], None),
    );

    // large.pdf — 200 numbered sections (performance and throughput tests).
    let pages: Vec<Content> = (1..=200)
        .map(|n| {
            let mut page = line("F2", 14, 72, 720, &format!("{n}. Seção {n}"));
            for (i, y) in [690u32, 676, 662, 648, 634, 620].into_iter().enumerate() {
                page.extend(line(
                    "F1",
                    11,
                    72,
                    y,
                    &format!(
                        "Linha {} da seção {n}: cláusulas, prazos e coberturas do contrato.",
                        i + 1
                    ),
                ));
            }
            Content::Text(page)
        })
        .collect();
    write(&dir, "large.pdf", document(pages, None));

    // encrypted.pdf — text.pdf protected with a user password ("segredo"), made with
    // Ghostscript when it is installed; otherwise the committed file is kept.
    let encrypted = dir.join("encrypted.pdf");
    match std::process::Command::new("gs")
        .args([
            "-q",
            "-dNOPAUSE",
            "-dBATCH",
            "-sDEVICE=pdfwrite",
            "-sOwnerPassword=dono",
            "-sUserPassword=segredo",
            "-dEncryptionR=3",
            "-dKeyLength=128",
        ])
        .arg(format!("-sOutputFile={}", encrypted.display()))
        .arg(dir.join("text.pdf"))
        .status()
    {
        Ok(status) if status.success() => println!("encrypted.pdf: written with Ghostscript"),
        _ => println!("encrypted.pdf: Ghostscript unavailable, keeping the committed file"),
    }

    // canary.pdf — a marker in the title and the text: privacy tests check that it never
    // reaches logs or measurements.
    let mut canary = line("F2", 14, 72, 720, "1. Dados sigilosos");
    canary.extend(line(
        "F1",
        11,
        72,
        700,
        "O paciente CANARIO-7f3a tem carência de 30 dias.",
    ));
    canary.extend(line(
        "F1",
        11,
        72,
        686,
        "Contato do titular: CANARIO-7f3a, plano ouro.",
    ));
    let mut canary_info = b"<< /Title ".to_vec();
    canary_info.extend(text("Prontuário CANARIO-7f3a"));
    canary_info.extend_from_slice(b" >>");
    write(
        &dir,
        "canary.pdf",
        document(vec![Content::Text(canary)], Some(canary_info)),
    );

    // corrupt.pdf — a PDF header followed by garbage.
    write(
        &dir,
        "corrupt.pdf",
        b"%PDF-1.7\nthis is not really a pdf\n".to_vec(),
    );
}

fn write(dir: &Path, name: &str, bytes: Vec<u8>) {
    fs::write(dir.join(name), &bytes).unwrap();
    println!("{name}: {} bytes", bytes.len());
}
