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

fn text_cell(reference: &str, value: &str) -> Cell {
    cell(reference, "inlineStr", value)
}

fn sheet(name: &str, rows: Vec<Vec<Cell>>) -> Sheet {
    Sheet {
        name: name.into(),
        state: "visible",
        rows,
    }
}

/// One sheet "Janeiro": the example of the product.
pub fn uma_aba() -> Vec<u8> {
    xlsx(
        &[],
        &[sheet(
            "Janeiro",
            vec![
                vec![
                    text_cell("A1", "Produto"),
                    text_cell("B1", "Quantidade"),
                    text_cell("C1", "Valor"),
                ],
                vec![
                    text_cell("A2", "Notebook"),
                    cell("B2", "n", "10"),
                    cell("C2", "n", "5000"),
                ],
            ],
        )],
        &[],
    )
}

/// Three months, one sheet each, the same columns.
pub fn varias_abas() -> Vec<u8> {
    let month = |name: &str, product: &str, qty: &str| {
        sheet(
            name,
            vec![
                vec![text_cell("A1", "Produto"), text_cell("B1", "Quantidade")],
                vec![text_cell("A2", product), cell("B2", "n", qty)],
            ],
        )
    };
    xlsx(
        &[],
        &[
            month("Janeiro", "Notebook", "10"),
            month("Fevereiro", "Monitor", "7"),
            month("Março", "Teclado", "25"),
        ],
        &[],
    )
}

pub fn cabecalho() -> Vec<u8> {
    xlsx(
        &[],
        &[sheet(
            "Equipe",
            vec![
                vec![
                    text_cell("A1", "Nome"),
                    text_cell("B1", "Cidade"),
                    text_cell("C1", "Cargo"),
                ],
                vec![
                    text_cell("A2", "João"),
                    text_cell("B2", "Fortaleza"),
                    text_cell("C2", "Engenheiro"),
                ],
                vec![
                    text_cell("A3", "Maria"),
                    text_cell("B3", "Recife"),
                    text_cell("C3", "Analista"),
                ],
            ],
        )],
        &[],
    )
}

/// Gaps: an empty middle cell, an empty last cell, a blank row and a column without a name.
pub fn vazias() -> Vec<u8> {
    xlsx(
        &[],
        &[sheet(
            "Lacunas",
            vec![
                vec![
                    text_cell("A1", "Nome"),
                    text_cell("B1", "Cidade"),
                    text_cell("D1", "Cargo"),
                ],
                vec![text_cell("A2", "Ana"), text_cell("D2", "Gerente")],
                vec![text_cell("A3", "Bia"), text_cell("B3", "Recife")],
                vec![
                    text_cell("A5", "Caio"),
                    text_cell("B5", "Natal"),
                    text_cell("C5", "sem nome de coluna"),
                    text_cell("D5", "Dev"),
                ],
            ],
        )],
        &[],
    )
}

pub fn numeros() -> Vec<u8> {
    xlsx(
        &[],
        &[sheet(
            "Números",
            vec![
                vec![text_cell("A1", "Tipo"), text_cell("B1", "Valor")],
                vec![text_cell("A2", "inteiro"), cell("B2", "n", "10")],
                vec![text_cell("A3", "decimal"), cell("B3", "n", "1250.5")],
                vec![text_cell("A4", "negativo"), cell("B4", "n", "-3")],
                vec![text_cell("A5", "científico"), cell("B5", "n", "1.5E-5")],
                vec![text_cell("A6", "grande"), cell("B6", "n", "12345678901")],
                vec![text_cell("A7", "fração"), cell("B7", "n", "0.1")],
                vec![text_cell("A8", "fórmula"), cell("B8", "fn", "6000")],
                vec![text_cell("A9", "booleano"), cell("B9", "b", "1")],
            ],
        )],
        &[],
    )
}

/// A date, a date with time and a number in a custom date format.
pub fn datas() -> Vec<u8> {
    xlsx(
        &[],
        &[sheet(
            "Datas",
            vec![
                vec![text_cell("A1", "Evento"), text_cell("B1", "Quando")],
                vec![text_cell("A2", "início"), styled("B2", "45352", 1)],
                vec![text_cell("A3", "reunião"), styled("B3", "45352.5", 1)],
                vec![text_cell("A4", "entrega"), styled("B4", "45383", 2)],
                vec![text_cell("A5", "versão"), styled("B5", "45352", 0)],
            ],
        )],
        &[],
    )
}

pub fn textos() -> Vec<u8> {
    xlsx(
        &["Descrição", "Ação & reação", "Cadeira de escritório"],
        &[sheet(
            "Textos",
            vec![
                vec![cell("A1", "s", "0"), text_cell("B1", "Observação")],
                vec![
                    cell("A2", "s", "1"),
                    text_cell("B2", "  espaços   extras  "),
                ],
                vec![
                    cell("A3", "s", "2"),
                    text_cell("B3", "linha um\nlinha dois"),
                ],
            ],
        )],
        &[],
    )
}

/// An Excel table (A3:C6) under a title, with a note after it.
pub fn tabela_excel() -> Vec<u8> {
    let rels = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/table\" Target=\"../tables/table1.xml\"/></Relationships>";
    let table = "<table xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" id=\"1\" name=\"Vendas\" displayName=\"Vendas\" ref=\"A3:C6\" headerRowCount=\"1\"/>";
    xlsx(
        &[],
        &[sheet(
            "Relatório",
            vec![
                vec![text_cell("A1", "Relatório de vendas 2024")],
                vec![
                    text_cell("A3", "Produto"),
                    text_cell("B3", "Região"),
                    text_cell("C3", "Total"),
                ],
                vec![
                    text_cell("A4", "Notebook"),
                    text_cell("B4", "Norte"),
                    cell("C4", "n", "100"),
                ],
                vec![
                    text_cell("A5", "Monitor"),
                    text_cell("B5", "Sul"),
                    cell("C5", "n", "200"),
                ],
                vec![
                    text_cell("A6", "Teclado"),
                    text_cell("B6", "Leste"),
                    cell("C6", "n", "300"),
                ],
                vec![text_cell("A8", "Valores em reais")],
            ],
        )],
        &[
            ("xl/worksheets/_rels/sheet1.xml.rels", rels.as_bytes()),
            ("xl/tables/table1.xml", table.as_bytes()),
        ],
    )
}

pub fn muitas_linhas() -> Vec<u8> {
    let mut rows = vec![vec![text_cell("A1", "Id"), text_cell("B1", "Item")]];
    for n in 2..=301u32 {
        rows.push(vec![
            cell(&format!("A{n}"), "n", &(n - 1).to_string()),
            text_cell(&format!("B{n}"), &format!("Item número {}", n - 1)),
        ]);
    }
    xlsx(&[], &[sheet("Itens", rows)], &[])
}

/// Every XLSX fixture file: `(file name, bytes)`.
pub fn xlsx_fixtures() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("uma-aba.xlsx", uma_aba()),
        ("varias-abas.xlsx", varias_abas()),
        ("cabecalho.xlsx", cabecalho()),
        ("vazias.xlsx", vazias()),
        ("numeros.xlsx", numeros()),
        ("datas.xlsx", datas()),
        ("textos.xlsx", textos()),
        ("tabela-excel.xlsx", tabela_excel()),
        ("muitas-linhas.xlsx", muitas_linhas()),
    ]
}

// ---- The golden corpus of the RAG tests (`tests/golden/corpus/`): documents with facts of their
// own and facts that complete those of the PDF, Markdown, TXT and CSV of the same corpus.

/// A manual: Arquitetura › Backend/Frontend, Deploy with a table.
pub fn operacoes() -> Vec<u8> {
    docx_with_core(
        &[
            p(Some("Ttulo1"), "Arquitetura"),
            p(None, "Plataforma Zeta com backend e frontend separados, serviços ligados por filas de mensagens."),
            p(Some("Ttulo2"), "Backend"),
            p(None, "O serviço de autenticação do backend emite tokens JWT com expiração de quinze minutos e renovação automática."),
            p(None, "A integração com o laboratório envia o resultado dos exames laboratoriais ao prontuário em até quatro horas."),
            li(1, 0, "Fila de eventos com reprocessamento"),
            li(1, 0, "Cache de sessões em memória"),
            p(Some("Ttulo2"), "Frontend"),
            p(None, "Interface web baseada na biblioteca Lumen, com tela de entrada em até dois segundos."),
            p(Some("Ttulo1"), "Deploy"),
            p(None, "O deploy em produção ocorre toda quinta-feira às 19h, com rollback automático em caso de falha."),
            p(None, "A renovação do certificado do painel é automatizada pelo pipeline de deploy a cada sessenta dias."),
            p(None, "O fornecedor Aurora também entrega o hardware dos servidores usados no deploy."),
            table_header(&[
                &["Ambiente", "Região", "Responsável"],
                &["Produção", "sa-east-1", "Marina"],
                &["Homologação", "us-east-1", "Carlos"],
            ]),
        ]
        .concat(),
        Some(&core(
            Some("Manual de Arquitetura e Deploy"),
            Some("Equipe de Plataforma"),
            None,
            None,
            None,
        )),
    )
}

/// A dashboard of indicators and a sheet of targets.
pub fn indicadores() -> Vec<u8> {
    let indicators = [
        ("Disponibilidade da plataforma", "99.9", "99.95", "%"),
        ("Satisfação do cliente", "90", "86", "%"),
        ("Tempo médio de resposta", "2", "2.4", "s"),
        ("Chamados abertos", "120", "98", "un"),
        ("Chamados resolvidos", "110", "104", "un"),
        ("Backups concluídos", "30", "30", "un"),
        ("Incidentes críticos", "0", "1", "un"),
        ("Cobertura de testes", "80", "76", "%"),
        ("Tempo de deploy", "15", "12", "min"),
        ("Erros por mil requisições", "2", "1.4", "un"),
        ("Usuários ativos por dia", "5000", "5230", "un"),
        ("Fila de eventos pendentes", "100", "40", "un"),
        ("Custo mensal de nuvem", "20000", "18900", "R$"),
        ("Alertas ruidosos", "10", "14", "un"),
        ("Atualizações de segurança aplicadas", "12", "12", "un"),
        ("Tempo de recuperação de falhas", "30", "25", "min"),
        ("Documentos indexados", "1000", "1180", "un"),
        ("Consultas respondidas com fonte", "95", "93", "%"),
        ("Treinamentos realizados", "4", "3", "un"),
        ("Revisões de acesso concluídas", "6", "6", "un"),
    ];
    let mut dashboard = vec![vec![
        text_cell("A1", "Indicador"),
        text_cell("B1", "Meta"),
        text_cell("C1", "Atual"),
        text_cell("D1", "Unidade"),
    ]];
    for (i, (name, goal, now, unit)) in indicators.iter().enumerate() {
        let r = i + 2;
        dashboard.push(vec![
            text_cell(&format!("A{r}"), name),
            cell(&format!("B{r}"), "n", goal),
            cell(&format!("C{r}"), "n", now),
            text_cell(&format!("D{r}"), unit),
        ]);
    }
    let goals = [
        ("Unidades vendidas de Cafe Torrado", "500", "por trimestre"),
        ("Casos omissos resolvidos pela operadora", "5", "dias úteis"),
        ("Entregas do fornecedor Aurora no prazo", "95", "por cento"),
    ];
    let mut metas = vec![vec![
        text_cell("A1", "Meta"),
        text_cell("B1", "Valor"),
        text_cell("C1", "Período"),
    ]];
    for (i, (name, value, period)) in goals.iter().enumerate() {
        let r = i + 2;
        metas.push(vec![
            text_cell(&format!("A{r}"), name),
            cell(&format!("B{r}"), "n", value),
            text_cell(&format!("C{r}"), period),
        ]);
    }
    let core = core(
        Some("Indicadores de Plataforma"),
        Some("Equipe de Plataforma"),
        None,
        None,
        None,
    );
    xlsx(
        &[],
        &[sheet("Dashboard", dashboard), sheet("Metas", metas)],
        &[("docProps/core.xml", core.as_bytes())],
    )
}

/// The golden-corpus files: `(file name, bytes)`.
pub fn golden_corpus() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("operacoes.docx", operacoes()),
        ("indicadores.xlsx", indicadores()),
    ]
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
                                "fn" => format!("<c r=\"{reference}\"{style}><f>SUM(A1:A2)</f><v>{value}</v></c>"),
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
    // A core part given in `extra` replaces the default one (a zip cannot hold two entries of a
    // name).
    if extra.iter().any(|(name, _)| *name == "docProps/core.xml") {
        entries.retain(|(name, _)| *name != "docProps/core.xml");
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
