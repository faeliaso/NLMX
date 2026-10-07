indexing-title = Indexação
indexing-subtitle = Extração, divisão em trechos e embeddings, em segundo plano.
indexing-unavailable-title = Indexação indisponível
indexing-empty-title = Nenhum documento indexado
indexing-empty-description = Ao importar documentos, o progresso de cada etapa é exibido aqui.
indexing-go-documents = Ir para Documentos
indexing-index-heading = Índice de busca
indexing-keyword-only-title = Busca só por palavras-chave
indexing-keyword-only-message = Sem um modelo de embeddings, as perguntas usam apenas a busca lexical. Ative um modelo para incluir a busca semântica.
indexing-open-models = Abrir Modelos
indexing-embedding-model = Modelo de embeddings
indexing-hybrid = Busca híbrida
indexing-none = Nenhum
indexing-keyword-only-badge = Só palavras-chave
indexing-embed-pending = Gerar embeddings pendentes
indexing-retry-failed = Tentar novamente as falhas
indexing-reindex-all = Reindexar tudo…
indexing-reindex-title = Reindexar todos os documentos?
indexing-reindex-description = Os vetores de { $documents } ({ $chunks }) serão gerados de novo com o modelo ativo. A busca continua funcionando enquanto isso.
indexing-reindex-confirm = Reindexar
indexing-cancel = Cancelar
indexing-attention-heading = Precisa de atenção
indexing-recent-heading = Concluídos recentemente
indexing-retry = Tentar novamente
indexing-disabled-while-running = Disponível quando a indexação em andamento terminar
indexing-stat-documents = Documentos
indexing-stat-chunks = Trechos
indexing-stat-dimensions = Dimensões dos vetores
indexing-chunks-count =
    { $count ->
        [one] { $count } trecho
       *[other] { $count } trechos
    }
indexing-attempts-count =
    { $count ->
        [one] { $count } tentativa
       *[other] { $count } tentativas
    }
indexing-documents-count =
    { $count ->
        [one] { $count } documento
       *[other] { $count } documentos
    }
indexing-docs-indexed =
    { $count ->
        [one] { $count } indexado
       *[other] { $count } indexados
    }
indexing-docs-reading = { $count } em leitura
indexing-docs-awaiting = { $count } aguardando embeddings
indexing-docs-no-text = { $count } sem texto
indexing-docs-failed = { $count } com falha
indexing-chunks-total = { $count } no total
indexing-chunks-embedded = { $count } com vetores
indexing-chunks-pending = { $count } aguardando
indexing-detail-waiting-model = Aguardando um modelo de embeddings para entrar na busca semântica.
indexing-detail-no-embeddings = Os embeddings ainda não foram gerados.
indexing-detail-needs-ocr = Nenhuma página tem texto selecionável. O reconhecimento de texto (OCR) ainda não está disponível.
indexing-duration-under-second = menos de 1 s
indexing-duration-seconds = { $seconds } s
indexing-duration-minutes = { $minutes } min { $seconds } s
