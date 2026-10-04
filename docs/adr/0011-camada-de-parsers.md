# ADR 0011 — Camada de parsers

- Status: aceito
- Data: 2026-10-04
- Atende: leitura de PDF, Markdown, TXT, CSV e EPUB com uma saída comum (continua o ADR 0010).

## Contexto
A ingestão só conhecia PDF: `DocumentEngine` → `StructureAnalyzer` → `Chunker`, com página e caixas obrigatórias em cada bloco. Para indexar outros formatos sem alterar o pipeline de RAG, cada formato precisa ser lido para uma representação intermediária única que preserve estrutura, metadados e localização até o chunking.

## Decisão
- **Port `DocumentParser`** (`application::ports`): `document_type`, `version`, `supports_type`, `supports_mime` e `parse(&DocumentSource) → ParsedDocument`. Um parser não conhece chunking, embeddings nem RAG. `ParseError` (domínio) tem mensagens sem conteúdo, nome ou caminho do arquivo.
- **`ParsedDocument`** (`domain::parsed`): metadados, `pages` (só PDF: tamanho, `has_text`; insumo do visualizador e do `NeedsOcr`), seções (`DocumentSection`: título, nível, caminho, blocos) e avisos (`ParseWarning`, sem conteúdo). Os blocos (`ContentBlock`) têm um tipo estrutural (`ContentKind`: título, parágrafo, item de lista, bloco de código, tabela, registro de CSV com o nome das colunas), o texto legível usado para embeddings e uma `SourceLocation`. Nada vira texto plano antes do chunking.
- **`ParserRegistry`** (`application::services::parsing`) escolhe o parser pelo formato declarado ou pela extensão; incluir um formato é registrar um parser na raiz de composição.
- **`PdfDocumentParser`** vive em `application`: compõe `DocumentEngine` (PDFium) e `StructureAnalyzer` e reaproveita `read_layouts`, extraída de `DocumentIngestion` (adapters não dependem uns dos outros). A extração do PDF não foi reescrita.
- **Adapters novos:** `parser-text` (`MarkdownDocumentParser`, `TextDocumentParser`, `CsvDocumentParser`; decodificação UTF-8/UTF-16 com BOM e fallback Windows-1252) e `parser-epub` (zip + XML; recusa DRM; limites contra zip bomb). Todos têm uma função síncrona pura e rodam em `spawn_blocking`.
- **CSV:** um `Record` por linha de dados (cabeçalho detectado ou colunas "coluna N"), com o número da linha; o chunker agrupará linhas repetindo o cabeçalho.
- Todo parser passa a mesma suíte de contrato (`nlmx_testing::document_parser_contract`).

## Consequências
- `DocumentIngestion`, `wiring.rs`, o chunker, o banco e a interface **não mudam nesta etapa**: os parsers existem, mas a ingestão ainda usa `DocumentEngine` direto. A próxima etapa liga `ParserRegistry` ao chunking e à persistência (coluna de localização, migração).
- `tests/tests/architecture.rs` passa a esperar 11 adapters.
- Um parágrafo sem quebras em TXT vira um bloco só; o chunker o divide e a localização do chunk herda o intervalo do bloco.

## Atualização: CSV semântico
- **Registro legível:** cada linha de dados vira um bloco `Record` cujo texto é `Registro N:` seguido de uma linha `Coluna: valor` por célula não vazia (espaços e quebras internas colapsados). Em `fields` ficam todas as colunas, inclusive as vazias. `N` é a linha de dados (o cabeçalho não conta), igual à localização `SourceLocation::Csv { row_start, row_end }`, que é o `Rows` pedido. O rótulo é "linhas N–M"; numa planilha o cabeçalho é a linha 1, então a linha da planilha é `N + 1` quando há cabeçalho.
- **Cabeçalho verificado:** é cabeçalho quando a primeira linha não tem número, data ou booleano, tem nomes distintos e há linhas abaixo. É *forte* quando há contraste de tipo com as linhas abaixo; num arquivo todo de texto só é aceito se as células forem curtas e únicas, e então o aviso `UncertainHeader` é emitido (a heurística é inerentemente ambígua nesse caso). Sem cabeçalho, as colunas se chamam "coluna N" (`NoHeaderRow`). Uma linha única é dado.
- **Dataset:** `DocumentMetadata.dataset` (`DatasetMetadata`: colunas com tipo — texto, número, data, booleano —, delimitador, se há cabeçalho e o número de linhas quando o arquivo foi lido por inteiro). Delimitadores `, ; tab |`; codificação UTF-8 (ou UTF-16 com BOM) com fallback Windows-1252.
- **Leitura incremental:** `CsvStream` (`parser-text`) valida a codificação em blocos, lê uma amostra do início para detectar delimitador, cabeçalho e tipos e entrega os registros um a um, com memória proporcional a um registro. `DocumentParser::parse` usa o mesmo fluxo, mas coleta até 250 mil linhas (`TooLarge` acima); o fluxo vai até 5 milhões de linhas e 1 GiB (`CsvLimits`).
- **`RecordChunker`** (`chunker-structural`, preguiçoso): agrupa registros até `target_tokens` com um preâmbulo (`Arquivo`, `Colunas`, `Linhas a–b`) em todo chunk, sem sobreposição (`overlap_tokens` é ignorado: registros são atômicos), funde uma cauda menor que `min_tokens`, trunca uma lista longa de colunas e divide por campos um registro maior que o orçamento ("Registro 7 (parte 2/3)"). O chunk é um `DocumentChunk` com `Csv { a, b }`, `file_name` e `columns`.
- O chunker ainda não é um port: nada o chama. O port e a escolha do chunker por formato entram com a etapa que religa `DocumentIngestion` a todos os formatos (migração, `fs-library`, `wiring.rs`).
