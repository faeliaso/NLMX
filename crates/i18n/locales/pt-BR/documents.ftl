documents-title = Documentos
documents-description = Documentos da sua biblioteca local. Nada sai deste Mac.
documents-import = Importar documentos
documents-add-note = Adicionar nota
documents-library-unavailable = Biblioteca indisponível
documents-empty-title = Nenhum documento
documents-empty-description = Use Importar documentos, no topo da página, para começar (PDF, Markdown, TXT, CSV/TSV, EPUB, DOCX ou XLSX), ou Adicionar nota para colar um texto. Os arquivos são copiados para a biblioteca local e divididos em trechos para a busca.
documents-note-dialog-title = Cole o texto copiado
documents-note-dialog-description = Cole o texto copiado abaixo para enviá-lo como uma fonte.
documents-note-placeholder = Cole ou escreva o texto da nota aqui
documents-note-label = Texto da nota
documents-note-insert = Inserir
documents-note-name = Nota
documents-open = Abrir
documents-details = Detalhes
documents-actions = Ações
documents-more-actions = Mais ações para { $title }
documents-more-actions-disabled-title = Disponível quando a leitura do documento terminar
documents-more-actions-disabled-label = Mais ações (indisponível enquanto o documento é processado)
documents-remove-menu = Remover…
documents-remove = Remover
documents-remove-dialog-title = Remover “{ $title }”?
documents-pages =
    { $count ->
        [one] { $count } página
       *[other] { $count } páginas
    }
documents-chunks =
    { $count ->
        [one] { $count } trecho
       *[other] { $count } trechos
    }
documents-date = { $day }/{ $month }/{ $year }
documents-status-embedding = Aguardando embeddings
documents-status-indexed = Indexado
documents-status-needs-ocr = Sem texto (OCR)
documents-status-failed = Falhou
documents-status-queued = Na fila
documents-status-extracting = Lendo o arquivo
documents-status-structuring = Estruturando
documents-status-chunking = Dividindo em trechos
documents-removal-base =
    { $chunks ->
        [0] A cópia do documento na biblioteca será apagada deste Mac.
        [1] A cópia do documento na biblioteca, seu único trecho e os índices de busca serão apagados deste Mac.
       *[other] A cópia do documento na biblioteca, seus { $chunks } trechos e os índices de busca serão apagados deste Mac.
    }
documents-removal-conversations =
    { $count ->
        [one] { $count } conversa será excluída
       *[other] { $count } conversas serão excluídas
    }
documents-removal-turns =
    { $count ->
        [one] { $count } par de pergunta e resposta que o usou será removido
       *[other] { $count } pares de pergunta e resposta que o usaram serão removidos
    }
documents-removal-history = No Chat, { $items }.
documents-removal-join = { $first } e { $second }
documents-removal-original = O arquivo original não é afetado.
documents-notice-removed = Documento removido. Ele e tudo o que derivava dele foram apagados deste Mac.
documents-notice-remove-failed = Não foi possível remover o documento: { $reason }
documents-notice-note-unavailable = Não foi possível adicionar a nota: a importação não está disponível.
documents-notice-note-added = Nota adicionada. Ela é indexada em segundo plano.
documents-picker-title = Importar documentos
documents-picker-filter = Documentos
documents-import-started =
    { $count ->
        [one] Importando { $count } arquivo…
       *[other] Importando { $count } arquivos…
    }
documents-import-imported =
    { $count ->
        [one] { $count } importado
       *[other] { $count } importados
    }
documents-import-duplicates =
    { $count ->
        [one] { $count } já estava na biblioteca
       *[other] { $count } já estavam na biblioteca
    }
documents-import-failed = { $count } com falha
documents-import-summary-failure = { $summary } — { $first }
documents-import-internal-register = falha interna ao registrar o arquivo
documents-import-internal-process = falha interna ao processar o arquivo
documents-import-internal-note = falha interna ao registrar a nota
