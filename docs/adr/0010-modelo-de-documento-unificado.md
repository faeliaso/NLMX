# ADR 0010 — Modelo de documento unificado

- Status: aceito
- Data: 2026-10-04
- Atende: evolução para PDF, Markdown, TXT, CSV e EPUB (primeira etapa: só o domínio).

## Contexto
Hoje o sistema só conhece PDF: não há um tipo para o formato do documento e a proveniência de um trecho é sempre `page_start`/`page_end` mais caixas em pontos de PDF (`ChunkDraft`, `ChunkView`, `MessageSource`, tabelas `document_chunks` e `citations`). Para indexar e responder a partir de outros formatos, o RAG precisa citar um trecho sem saber como o documento foi lido, e só o PDF tem visualizador.

## Decisão
- Novos tipos em `nlmx-domain`, aditivos (nenhum tipo existente muda de campos nem de comportamento):
  - `document_type::DocumentType` (`Pdf`, `Markdown`, `Text`, `Csv`, `Epub`): nome estável (`as_str`, igual ao serde), `from_extension`/`extensions` como fonte única das extensões aceitas, `previewable()` e `is_paged()`.
  - `source::SourceLocation`: enum com uma variante por formato (`Pdf` página + caixas opcionais; `Markdown` caminho de títulos + linhas opcionais; `Text` intervalo `[start, end)` em caracteres do texto decodificado; `Csv` linhas de dados 1-based sem o cabeçalho; `Epub` capítulo 1-based + título/seção opcionais). Construtores validados e `validate()` para valores desserializados; `label()` em português para citações.
  - `source::SourceReference`: documento, título, trecho, `SourceLocation` e caminho de seção. O tipo e a visualização **derivam da localização**, então não há tipo e localização em desacordo.
  - `parsed::{Document, DocumentMetadata, DocumentSection, ParsedDocument, DocumentChunk}`: a saída neutra de qualquer parser (a estrutura em blocos tipados está detalhada no ADR 0011). `ParsedDocument::new` exige que toda seção e bloco sejam do formato declarado. `DocumentChunk::from_draft` lê um `ChunkDraft` de PDF como chunk neutro (visão de mão única).
- **`previewable` não decide indexação.** Todo `DocumentType` pode ser indexado e usado pelo RAG; `previewable` (só PDF) diz apenas se existe visualizador. Não existe `is_indexable`.
- O domínio passa a depender de `serde` (derive) para `DocumentType`, `SourceLocation` e os tipos acima, e de `serde_json` só em testes. `BoundingBox` e `PageBox` ganham `Serialize`/`Deserialize`. Continua sem depender de crates do workspace ou de infraestrutura (`tests/tests/architecture.rs`).
- `document::DocumentMetadata` (PDF, com `pdf_version` e `page_count`) permanece como está; `parsed::DocumentMetadata` é o tipo neutro, em outro módulo.

## Consequências
- Nada usa os tipos novos ainda: ports, banco, RAG e interface seguem só PDF. As próximas etapas (coluna `locator` com a `SourceLocation` em JSON, port de extração, citações neutras) os consomem.
- Citações de formatos sem visualizador não são clicáveis: mostram o rótulo da localização.
- Mudar o formato JSON de `SourceLocation` depois de gravá-lo no banco exige migração; o formato está fixado por testes.
