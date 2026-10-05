//! Builds DOCX and XLSX files in memory, for the tests and for the fixture generator.
//! Deterministic: fixed timestamps.

#![allow(dead_code)]

use std::io::{Cursor, Write};

use zip::{CompressionMethod, DateTime, ZipWriter, write::SimpleFileOptions};

pub fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let time = DateTime::from_date_and_time(2024, 1, 1, 0, 0, 0).unwrap();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(time);
        writer.start_file(*name, options).unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

const W: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";

pub fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `<w:p>` with an optional style.
pub fn p(style: Option<&str>, text: &str) -> String {
    let style = style
        .map(|s| format!("<w:pPr><w:pStyle w:val=\"{s}\"/></w:pPr>"))
        .unwrap_or_default();
    format!(
        "<w:p>{style}<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
        esc(text)
    )
}

/// A list item (`num` 1 is bulleted, 2 is numbered in `NUMBERING`).
pub fn li(num: u32, level: u32, text: &str) -> String {
    format!(
        "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"{level}\"/><w:numId w:val=\"{num}\"/></w:numPr></w:pPr><w:r><w:t>{}</w:t></w:r></w:p>",
        esc(text)
    )
}

pub fn table(rows: &[&[&str]]) -> String {
    let rows: String = rows
        .iter()
        .map(|row| {
            let cells: String = row
                .iter()
                .map(|c| format!("<w:tc>{}</w:tc>", p(None, c)))
                .collect();
            format!("<w:tr>{cells}</w:tr>")
        })
        .collect();
    format!("<w:tbl>{rows}</w:tbl>")
}

pub const STYLES: &str = "<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:style w:type=\"paragraph\" w:styleId=\"Ttulo1\"><w:name w:val=\"heading 1\"/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Ttulo2\"><w:name w:val=\"heading 2\"/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Ttulo3\"><w:name w:val=\"heading 3\"/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Legenda\"><w:name w:val=\"caption\"/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Subtitulo\"><w:name w:val=\"Meu estilo\"/><w:basedOn w:val=\"Ttulo2\"/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style></w:styles>";

pub const NUMBERING: &str = "<w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:abstractNum w:abstractNumId=\"0\"><w:lvl w:ilvl=\"0\"><w:numFmt w:val=\"bullet\"/></w:lvl></w:abstractNum>\
<w:abstractNum w:abstractNumId=\"1\"><w:lvl w:ilvl=\"0\"><w:numFmt w:val=\"decimal\"/></w:lvl></w:abstractNum>\
<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>\
<w:num w:numId=\"2\"><w:abstractNumId w:val=\"1\"/></w:num></w:numbering>";

const CORE: &str = "<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\"><dc:title>Contrato de Serviços</dc:title><dc:creator>Ana Souza</dc:creator><dc:language>pt-BR</dc:language><dcterms:created>2024-03-01T10:00:00Z</dcterms:created></cp:coreProperties>";

const ROOT_RELS: &str = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>";

/// A DOCX around `body` (the content of `<w:body>`), with the default core properties.
pub fn docx(body: &str) -> Vec<u8> {
    docx_with_core(body, Some(CORE))
}

/// Like [`docx`] with the given `docProps/core.xml` (none when `core` is `None`).
pub fn docx_with_core(body: &str, core: Option<&str>) -> Vec<u8> {
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><w:document {W} xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><w:body>{body}</w:body></w:document>"
    );
    docx_from_document(document.as_bytes(), core)
}

/// A DOCX whose `word/document.xml` is exactly `document` (any encoding).
pub fn docx_from_document(document: &[u8], core: Option<&str>) -> Vec<u8> {
    let mut entries: Vec<(&str, &[u8])> = vec![
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("word/document.xml", document),
        ("word/styles.xml", STYLES.as_bytes()),
        ("word/numbering.xml", NUMBERING.as_bytes()),
    ];
    if let Some(core) = core {
        entries.push(("docProps/core.xml", core.as_bytes()));
    }
    zip(&entries)
}

/// `docProps/core.xml` with the given fields (`None` leaves a field out).
pub fn core(
    title: Option<&str>,
    creator: Option<&str>,
    subject: Option<&str>,
    created: Option<&str>,
    modified: Option<&str>,
) -> String {
    let field = |tag: &str, value: Option<&str>| {
        value
            .map(|v| format!("<{tag}>{}</{tag}>", esc(v)))
            .unwrap_or_default()
    };
    format!(
        "<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\">{}{}{}{}{}</cp:coreProperties>",
        field("dc:title", title),
        field("dc:creator", creator),
        field("dc:subject", subject),
        field("dcterms:created", created),
        field("dcterms:modified", modified),
    )
}

/// A paragraph made of `runs` (already XML).
pub fn para(runs: &str) -> String {
    format!("<w:p>{runs}</w:p>")
}

pub fn run(text: &str) -> String {
    format!("<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>", esc(text))
}

/// A hyperlink around `text`.
pub fn hyperlink(text: &str) -> String {
    format!("<w:hyperlink r:id=\"rId9\">{}</w:hyperlink>", run(text))
}

/// A paragraph that ends a Word section (`w:sectPr`), as Word writes between sections.
pub fn section_break(text: &str) -> String {
    format!(
        "<w:p><w:pPr><w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/></w:sectPr></w:pPr>{}</w:p>",
        run(text)
    )
}

/// A table whose first row is declared a header (`w:tblHeader`).
pub fn table_header(rows: &[&[&str]]) -> String {
    let rows: String = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let props = if i == 0 {
                "<w:trPr><w:tblHeader/></w:trPr>"
            } else {
                ""
            };
            let cells: String = row
                .iter()
                .map(|c| format!("<w:tc>{}</w:tc>", p(None, c)))
                .collect();
            format!("<w:tr>{props}{cells}</w:tr>")
        })
        .collect();
    format!("<w:tbl>{rows}</w:tbl>")
}

/// A one-row table whose first cell spans `span` columns.
pub fn merged_row(first: &str, span: u32, rest: &[&str]) -> String {
    let first = format!(
        "<w:tc><w:tcPr><w:gridSpan w:val=\"{span}\"/></w:tcPr>{}</w:tc>",
        p(None, first)
    );
    let rest: String = rest
        .iter()
        .map(|c| format!("<w:tc>{}</w:tc>", p(None, c)))
        .collect();
    format!("<w:tr>{first}{rest}</w:tr>")
}

// ---- Fixtures: one small file per scenario.

pub fn simples() -> Vec<u8> {
    docx(
        &[
            p(None, "Primeiro parágrafo, sem estrutura."),
            p(None, "Segundo parágrafo com acentuação: ação, coração."),
        ]
        .concat(),
    )
}

pub fn titulos() -> Vec<u8> {
    docx(
        &[
            p(Some("Ttulo1"), "Arquitetura"),
            p(None, "Visão geral do sistema."),
            p(Some("Ttulo2"), "Backend"),
            p(None, "Serviços e filas."),
            p(Some("Ttulo3"), "Autenticação"),
            p(None, "Tokens de acesso e renovação."),
            p(Some("Ttulo2"), "Frontend"),
            p(None, "Interface web."),
        ]
        .concat(),
    )
}

pub fn listas() -> Vec<u8> {
    docx(
        &[
            p(None, "Requisitos:"),
            li(1, 0, "Autenticação"),
            li(1, 0, "Autorização"),
            p(None, "Passos:"),
            li(2, 0, "Instalar"),
            li(2, 0, "Configurar"),
        ]
        .concat(),
    )
}

pub fn tabela() -> Vec<u8> {
    docx(
        &[
            p(Some("Ttulo1"), "Equipe"),
            table_header(&[
                &["Nome", "Cidade", "Cargo"],
                &["João", "Fortaleza", "Engenheiro"],
                &["Maria", "Recife", "Analista"],
            ]),
        ]
        .concat(),
    )
}

pub fn links() -> Vec<u8> {
    docx(&para(
        &[
            run("Veja a "),
            hyperlink("documentação oficial"),
            run(" para detalhes."),
        ]
        .concat(),
    ))
}

pub fn secoes() -> Vec<u8> {
    docx(
        &[
            p(Some("Ttulo1"), "Parte um"),
            section_break("Fim da primeira seção."),
            p(Some("Ttulo1"), "Parte dois"),
            p(None, "Conteúdo da segunda seção."),
            section_break("Fim da segunda seção."),
            p(Some("Ttulo1"), "Parte três"),
            p(None, "Conteúdo final."),
        ]
        .concat(),
    )
}

pub fn metadados() -> Vec<u8> {
    docx_with_core(
        &p(None, "Texto qualquer."),
        Some(&core(
            Some("Manual do Sistema"),
            Some("Ana Souza"),
            Some("Operação"),
            Some("2024-03-01T10:00:00Z"),
            Some("2024-05-20T16:30:00Z"),
        )),
    )
}

/// A long paragraph (about 60 words) whose first word is `marker`.
pub fn long_text(marker: &str) -> String {
    let mut text = format!("{marker} descreve o comportamento esperado do componente.");
    for n in 1..=7 {
        text.push_str(&format!(
            " A frase {n} de {marker} explica como os módulos se comunicam entre si."
        ));
    }
    text
}

/// The example of the product: Introdução, Arquitetura › Backend/Frontend, Deploy, with a list in
/// Backend, a captioned table in Frontend and a second table in Deploy.
pub fn manual() -> Vec<u8> {
    docx(
        &[
            p(Some("Ttulo1"), "Introdução"),
            p(None, &long_text("Introducao")),
            p(Some("Ttulo1"), "Arquitetura"),
            p(None, &long_text("Visaogeral")),
            p(Some("Ttulo2"), "Backend"),
            p(None, &long_text("Servicos")),
            p(None, &long_text("Filas")),
            p(None, &long_text("Banco")),
            li(1, 0, "Autenticação por token"),
            li(1, 0, "Autorização por papéis"),
            p(None, &long_text("Observabilidade")),
            p(Some("Ttulo2"), "Frontend"),
            p(Some("Legenda"), "Tabela 1 – Telas principais"),
            table_header(&[
                &["Tela", "Responsável", "Estado"],
                &["Login", "Marina", "Pronta"],
                &["Painel", "Carlos", "Em revisão"],
            ]),
            p(None, &long_text("Interface")),
            p(Some("Ttulo1"), "Deploy"),
            table_header(&[
                &["Ambiente", "Região"],
                &["Produção", "sa-east-1"],
                &["Homologação", "us-east-1"],
            ]),
            p(None, &long_text("Entrega")),
        ]
        .concat(),
    )
}

/// Every fixture file: `(file name, bytes)`.
pub fn fixtures() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("simples.docx", simples()),
        ("titulos.docx", titulos()),
        ("listas.docx", listas()),
        ("tabela.docx", tabela()),
        ("links.docx", links()),
        ("secoes.docx", secoes()),
        ("metadados.docx", metadados()),
        ("manual.docx", manual()),
    ]
}

pub fn contrato() -> Vec<u8> {
    let body = [
        p(None, "Minuta preliminar sujeita a revisão."),
        p(Some("Ttulo1"), "Objeto"),
        p(
            None,
            "O presente contrato trata da prestação de serviços de manutenção predial.",
        ),
        p(Some("Ttulo2"), "Prazos"),
        p(
            None,
            "O prazo de vigência é de vinte e quatro meses & renovável.",
        ),
        li(1, 0, "Aviso prévio de trinta dias"),
        li(2, 0, "Multa de dez por cento"),
        table(&[
            &["Parcela", "Valor"],
            &["Entrada", "R$ 1.000"],
            &["Final", "R$ 2.000"],
        ]),
        p(Some("Ttulo1"), "Disposições gerais"),
        p(Some("Subtitulo"), "Foro"),
        p(None, "Fica eleito o foro da comarca de Fortaleza."),
    ]
    .concat();
    docx(&body)
}

/// One sheet: `rows` of `(cell reference, type, value)`; `t` is `s` (shared index), `n`, `b`,
/// `str`, or `inlineStr`.
/// `(reference, type, value, style)` of one cell.
pub type Cell = (String, &'static str, String, Option<u32>);

pub struct Sheet {
    pub name: String,
    pub state: &'static str,
    pub rows: Vec<Vec<Cell>>,
}

pub fn cell(reference: &str, kind: &'static str, value: &str) -> Cell {
    (reference.into(), kind, value.into(), None)
}

pub fn styled(reference: &str, value: &str, style: u32) -> Cell {
    (reference.into(), "n", value.into(), Some(style))
}

const SS_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

/// An XLSX with `shared` strings and the given sheets.
pub fn xlsx(shared: &[&str], sheets: &[Sheet], extra: &[(&str, &[u8])]) -> Vec<u8> {
    let sst: String = shared
        .iter()
        .map(|s| format!("<si><t>{}</t></si>", esc(s)))
        .collect();
    let sst = format!("<sst xmlns=\"{SS_NS}\">{sst}</sst>");
    let styles = format!(
        "<styleSheet xmlns=\"{SS_NS}\"><numFmts count=\"1\"><numFmt numFmtId=\"164\" formatCode=\"dd/mm/yyyy\"/></numFmts><cellXfs count=\"3\"><xf numFmtId=\"0\"/><xf numFmtId=\"14\"/><xf numFmtId=\"164\"/></cellXfs></styleSheet>"
    );
    let workbook_sheets: String = sheets
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "<sheet name=\"{}\" sheetId=\"{}\" state=\"{}\" r:id=\"rId{}\"/>",
                esc(&s.name),
                i + 1,
                s.state,
                i + 1
            )
        })
        .collect();
    let workbook = format!(
        "<workbook xmlns=\"{SS_NS}\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>{workbook_sheets}</sheets></workbook>"
    );
    let rels: String = (0..sheets.len())
        .map(|i| {
            format!(
                "<Relationship Id=\"rId{0}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{0}.xml\"/>",
                i + 1
            )
        })
        .collect();
    let rels = format!(
        "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{rels}</Relationships>"
    );
    let root = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>";
    let sheet_xml: Vec<(String, Vec<u8>)> = sheets
        .iter()
        .enumerate()
        .map(|(i, sheet)| {
            let rows: String = sheet
                .rows
                .iter()
                .map(|row| {
                    let cells: String = row
                        .iter()
                        .map(|(reference, kind, value, style)| {
                            let style = style.map(|s| format!(" s=\"{s}\"")).unwrap_or_default();
                            match *kind {
                                "inlineStr" => format!("<c r=\"{reference}\" t=\"inlineStr\"{style}><is><t>{}</t></is></c>", esc(value)),
                                "n" => format!("<c r=\"{reference}\"{style}><v>{value}</v></c>"),
                                kind => format!("<c r=\"{reference}\" t=\"{kind}\"{style}><v>{}</v></c>", esc(value)),
                            }
                        })
                        .collect();
                    let number: String = row
                        .first()
                        .map(|c| c.0.chars().filter(char::is_ascii_digit).collect())
                        .unwrap_or_default();
                    format!("<row r=\"{number}\">{cells}</row>")
                })
                .collect();
            (
                format!("xl/worksheets/sheet{}.xml", i + 1),
                format!("<worksheet xmlns=\"{SS_NS}\"><sheetData>{rows}</sheetData></worksheet>").into_bytes(),
            )
        })
        .collect();
    let mut entries: Vec<(&str, &[u8])> = vec![
        ("_rels/.rels", root.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
        ("xl/sharedStrings.xml", sst.as_bytes()),
        ("xl/styles.xml", styles.as_bytes()),
        ("docProps/core.xml", CORE.as_bytes()),
    ];
    for (name, bytes) in &sheet_xml {
        entries.push((name, bytes));
    }
    entries.extend_from_slice(extra);
    zip(&entries)
}

/// Shared strings of `vendas()`.
pub const VENDAS_STRINGS: [&str; 6] =
    ["Região", "Produto", "Vendas", "Nordeste", "Cadeira", "Mesa"];

pub fn vendas() -> Vec<u8> {
    let resumo = Sheet {
        name: "Resumo".into(),
        state: "visible",
        rows: vec![
            vec![
                cell("A1", "s", "0"),
                cell("B1", "s", "1"),
                cell("C1", "s", "2"),
                cell("D1", "s", "0"),
            ],
            vec![
                cell("A2", "s", "3"),
                cell("B2", "s", "4"),
                cell("C2", "n", "1250.5"),
                styled("D2", "45352", 1),
            ],
            vec![
                cell("A4", "s", "3"),
                cell("B4", "s", "5"),
                cell("C4", "n", "980"),
                styled("D4", "45353", 2),
            ],
        ],
    };
    let oculta = Sheet {
        name: "Oculta".into(),
        state: "hidden",
        rows: vec![vec![cell("A1", "inlineStr", "segredo interno")]],
    };
    let notas = Sheet {
        name: "Notas".into(),
        state: "visible",
        rows: vec![
            vec![cell("A1", "inlineStr", "Observação")],
            vec![cell("A2", "inlineStr", "Meta revisada no trimestre")],
        ],
    };
    xlsx(&VENDAS_STRINGS, &[resumo, oculta, notas], &[])
}
