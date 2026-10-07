//! Builds EPUBs in memory, for the tests and for the fixture generator. Deterministic:
//! fixed timestamps, `mimetype` first and stored.

#![allow(dead_code)]

use std::io::{Cursor, Write};

use zip::{CompressionMethod, DateTime, ZipWriter, write::SimpleFileOptions};

/// A zip with the given entries in order; `mimetype` is stored, the rest deflated.
pub fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let time = DateTime::from_date_and_time(2024, 1, 1, 0, 0, 0).unwrap();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        let method = if *name == "mimetype" {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        let options = SimpleFileOptions::default()
            .compression_method(method)
            .last_modified_time(time);
        writer.start_file(*name, options).unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// A complete XHTML page around `body`.
pub fn page(body: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n<head><title>t</title></head>\n<body>\n{body}\n</body>\n</html>\n"
    )
    .into_bytes()
}

pub struct Chapter {
    /// File name inside the content directory.
    pub file: String,
    pub content: Vec<u8>,
    pub linear: bool,
    /// What the manifest says when it differs from `file` (e.g. percent-escapes).
    pub href: Option<String>,
}

impl Chapter {
    pub fn new(file: &str, body: &str) -> Self {
        Self {
            file: file.into(),
            content: page(body),
            linear: true,
            href: None,
        }
    }

    pub fn raw(file: &str, content: &[u8]) -> Self {
        Self {
            file: file.into(),
            content: content.to_vec(),
            linear: true,
            href: None,
        }
    }
}

pub struct Book {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub date: Option<String>,
    pub subjects: Vec<String>,
    pub chapters: Vec<Chapter>,
    /// `(file, title)` entries of the EPUB 3 navigation document.
    pub nav: Option<Vec<(String, String)>>,
    /// `(file, title)` entries of the EPUB 2 NCX.
    pub ncx: Option<Vec<(String, String)>>,
    /// `Algorithm` of each entry of `META-INF/encryption.xml`.
    pub encryption: Vec<String>,
    /// Chapters listed in the manifest and the spine whose file is not in the zip.
    pub missing_files: Vec<String>,
    pub extra_files: Vec<(String, Vec<u8>)>,
    pub opf_dir: String,
}

impl Default for Book {
    fn default() -> Self {
        Self {
            title: Some("Livro de Teste".into()),
            authors: vec![],
            language: None,
            publisher: None,
            date: None,
            subjects: vec![],
            chapters: vec![],
            nav: None,
            ncx: None,
            encryption: vec![],
            missing_files: vec![],
            extra_files: vec![],
            opf_dir: "OEBPS".into(),
        }
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;")
}

impl Book {
    pub fn new(chapters: Vec<Chapter>) -> Self {
        Self {
            chapters,
            ..Self::default()
        }
    }

    pub fn build(&self) -> Vec<u8> {
        let dir = &self.opf_dir;
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut entries: Vec<(String, Vec<u8>)> = vec![
            ("mimetype".into(), b"application/epub+zip".to_vec()),
            (
                "META-INF/container.xml".into(),
                format!(
                    "<?xml version=\"1.0\"?>\n<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n<rootfiles><rootfile full-path=\"{prefix}content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles>\n</container>\n"
                )
                .into_bytes(),
            ),
        ];
        if !self.encryption.is_empty() {
            let methods: String = self
                .encryption
                .iter()
                .map(|a| {
                    format!("<EncryptedData xmlns=\"http://www.w3.org/2001/04/xmlenc#\"><EncryptionMethod Algorithm=\"{a}\"/><CipherData><CipherReference URI=\"{prefix}fonts/a.otf\"/></CipherData></EncryptedData>")
                })
                .collect();
            entries.push((
                "META-INF/encryption.xml".into(),
                format!("<?xml version=\"1.0\"?>\n<encryption xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">{methods}</encryption>\n").into_bytes(),
            ));
        }

        let mut metadata = String::from("<dc:identifier id=\"id\">urn:uuid:0000</dc:identifier>");
        if let Some(title) = &self.title {
            metadata += &format!("<dc:title>{}</dc:title>", escape(title));
        }
        for author in &self.authors {
            metadata += &format!("<dc:creator>{}</dc:creator>", escape(author));
        }
        if let Some(language) = &self.language {
            metadata += &format!("<dc:language>{language}</dc:language>");
        }
        if let Some(publisher) = &self.publisher {
            metadata += &format!("<dc:publisher>{}</dc:publisher>", escape(publisher));
        }
        if let Some(date) = &self.date {
            metadata += &format!("<dc:date>{date}</dc:date>");
        }
        for subject in &self.subjects {
            metadata += &format!("<dc:subject>{}</dc:subject>", escape(subject));
        }

        let mut manifest = String::new();
        let mut spine = String::new();
        if self.nav.is_some() {
            manifest += "<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>";
        }
        if self.ncx.is_some() {
            manifest +=
                "<item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>";
        }
        let listed = self
            .chapters
            .iter()
            .map(|c| c.href.as_deref().unwrap_or(&c.file))
            .chain(self.missing_files.iter().map(String::as_str));
        for (i, file) in listed.enumerate() {
            let linear = self.chapters.get(i).is_none_or(|c| c.linear);
            manifest += &format!(
                "<item id=\"c{i}\" href=\"{file}\" media-type=\"application/xhtml+xml\"/>"
            );
            spine += &format!(
                "<itemref idref=\"c{i}\"{}/>",
                if linear { "" } else { " linear=\"no\"" }
            );
        }
        let toc_attr = if self.ncx.is_some() {
            " toc=\"ncx\""
        } else {
            ""
        };
        entries.push((
            format!("{prefix}content.opf"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"id\">\n<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">{metadata}</metadata>\n<manifest>{manifest}</manifest>\n<spine{toc_attr}>{spine}</spine>\n</package>\n"
            )
            .into_bytes(),
        ));

        if let Some(nav) = &self.nav {
            let items: String = nav
                .iter()
                .map(|(file, title)| format!("<li><a href=\"{file}\">{}</a></li>", escape(title)))
                .collect();
            entries.push((
                format!("{prefix}nav.xhtml"),
                page(&format!("<nav epub:type=\"toc\"><ol>{items}</ol></nav>")),
            ));
        }
        if let Some(ncx) = &self.ncx {
            let points: String = ncx
                .iter()
                .enumerate()
                .map(|(i, (file, title))| {
                    format!("<navPoint id=\"n{i}\" playOrder=\"{}\"><navLabel><text>{}</text></navLabel><content src=\"{file}\"/></navPoint>", i + 1, escape(title))
                })
                .collect();
            entries.push((
                format!("{prefix}toc.ncx"),
                format!("<?xml version=\"1.0\"?>\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\"><navMap>{points}</navMap></ncx>\n").into_bytes(),
            ));
        }
        for chapter in &self.chapters {
            entries.push((format!("{prefix}{}", chapter.file), chapter.content.clone()));
        }
        for (name, data) in &self.extra_files {
            entries.push((format!("{prefix}{name}"), data.clone()));
        }
        let refs: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_slice()))
            .collect();
        zip(&refs)
    }
}

/// The book of the committed fixture `livro.epub`.
pub fn livro() -> Book {
    let mut book = Book::new(vec![
        Chapter::new(
            "ch1.xhtml",
            "<h1>Partida</h1>\n<p>A viagem de inverno come&ccedil;ou numa manh&atilde; fria, com neve nas montanhas.</p>\n<p>Levamos caf&eacute; &amp; p&atilde;o&nbsp;quente na mochila.</p>\n<ul><li>malas</li><li>mapa</li></ul>",
        ),
        Chapter::new(
            "ch2.xhtml",
            "<h1>Travessia</h1>\n<p>O mar estava calmo.</p>\n<h2>O mar</h2>\n<p>As águas eram escuras e profundas.</p>\n<h3>As ondas</h3>\n<p>Cada onda trazia sal e silêncio.</p>\n<h2>O vento</h2>\n<p>O vento vinha do norte.</p>\n<pre><code class=\"language-python\">print(\"olá, mar\")\n</code></pre>",
        ),
        Chapter::new(
            "ch3.xhtml",
            "<p>Chegamos ao porto ao amanhecer.</p>\n<table><thead><tr><th>Porto</th><th>Dias</th></tr></thead><tbody><tr><td>Santos</td><td>12</td></tr></tbody></table>",
        ),
    ]);
    book.title = Some("A Viagem de Inverno".into());
    book.authors = vec!["Ana Souza".into()];
    book.language = Some("pt-BR".into());
    book.publisher = Some("Editora Exemplo".into());
    book.date = Some("2023-05-01".into());
    book.subjects = vec!["Viagens".into()];
    book.nav = Some(vec![
        ("ch1.xhtml".into(), "Partida".into()),
        ("ch2.xhtml".into(), "Travessia".into()),
        ("ch3.xhtml".into(), "Chegada".into()),
    ]);
    book
}
