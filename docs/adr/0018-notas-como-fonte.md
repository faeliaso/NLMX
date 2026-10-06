# ADR 0018 — Notas: texto colado como fonte, sem arquivo

- Status: aceito
- Data: 2026-10-05
- Atende: RF01 (novas formas de adicionar conhecimento). Estende os ADRs 0011–0017; não substitui nenhum.

## Contexto
Todo conhecimento entrava por arquivo (`<sha>.<ext>` na biblioteca). O usuário também quer colar um texto copiado e usá-lo como fonte, com o mesmo tratamento de qualquer documento: normalização, chunking, embeddings, SQLite, sqlite-vec e RAG.

## Decisão
- **Um `DocumentType::Note`** (`note`, "Nota"), nunca `previewable` nem `is_paged`. Não existe viewer, editor nem preview de nota: ela abre o painel **Informações da fonte** (ADR 0016), sem "Arquivo" nem "Tamanho". A extensão `nlmx-note` e o MIME `application/x-nlmx-note` são reservados só para manter as buscas totais; nunca são oferecidos no seletor de arquivos.
- **Sem pipeline paralelo.** `DocumentIngestion::enqueue_note` registra o documento (`queued`) e o resto é o `ContentPipeline` de sempre (`parse → normalize → chunk`, depois `EmbedDocuments`). A nota usa a mesma fila (`ImportQueue`), os mesmos eventos (`documents-changed`, `import-finished`), o mesmo `IndexingActivity`, o mesmo `RemoveDocument`.
- **Sem arquivo físico: o texto fica no SQLite.** Migration `0014_note`: troca da coluna `documents.format` (CHECK com `'note'`, como a 0013, nunca recriando `documents`) e `documents.note_text TEXT`. `library_path` fica vazio e `original_path` é `NULL`. Reindexar, retentar e retomar a indexação no startup leem o texto do banco: `DocumentSource` ganhou `text: Option<Arc<str>>` (`DocumentSource::note`) e o parser nunca lê um caminho. A remoção já cobre a coluna (cascata + `secure_delete`); `FileStore::remove` de um arquivo inexistente não é erro.
- **Parser.** `NoteDocumentParser` (`parser-text`) reaproveita `TextDocumentParser::parse_bytes` (parágrafos por linhas em branco, parágrafo longo dividido) e reescreve as localizações para `SourceLocation::Note { start, end }`: intervalo de caracteres do texto da nota, como `Text`. `Text` não serve porque `save_processed` exige que a localização combine com o formato do documento. Sem página, planilha ou arquivo inventados; o rótulo de uma fonte é `<título> · caracteres a–b`.
- **Texto e título (`domain::note`).** `clean_note_text` unifica `\n`, tira espaços no fim das linhas e linhas em branco nas pontas e repete-as no máximo uma vez; recusa vazio/só espaços (`NoteError::Empty`) e mais de 1 milhão de caracteres. O título é a primeira linha quando parece título (curta, sem pontuação final de frase; `#` é removido), senão o começo do conteúdo; fica em `documents.title`/`original_filename` e não passa por `title_from_filename`. Não há campo de título no modal.
- **Duplicata.** O "hash" de uma nota é o SHA-256 do texto limpo com prefixo de separação de domínio (`FileStore::digest_text`), então a mesma nota é `Duplicate` e uma nota nunca colide com um arquivo de mesmos bytes.
- **Interface.** O botão "Adicionar nota" fica ao lado de "Importar" e abre um `<dialog>` com `textarea`, Cancelar e Inserir (desabilitado sem texto não-branco; um só envio). O envio é `POST /documents/notes` (HTMX), que chama a porta `NoteSubmitter` (implementada em `src-tauri` sobre a `ImportQueue`); o resultado é um toast e a nota aparece em Documentos como `queued → … → indexed`.

## Consequências
- Nenhum formato existente muda de comportamento; todo `match` exaustivo sobre `DocumentType`/`SourceLocation` ganhou o braço de nota.
- Editar uma nota não existe (o hash mudaria): cola-se outra.
- Os contratos de repositório (`document_store_contract`) passaram a cobrir todos os formatos, incluindo `note`.
