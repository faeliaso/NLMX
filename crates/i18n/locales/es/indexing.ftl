indexing-title = Indexación
indexing-subtitle = Extracción, división en fragmentos y embeddings, en segundo plano.
indexing-unavailable-title = Indexación no disponible
indexing-empty-title = Ningún documento indexado
indexing-empty-description = Al importar documentos, aquí se muestra el progreso de cada etapa.
indexing-go-documents = Ir a Documentos
indexing-index-heading = Índice de búsqueda
indexing-keyword-only-title = Búsqueda solo por palabras clave
indexing-keyword-only-message = Sin un modelo de embeddings, las preguntas usan solo la búsqueda léxica. Activa un modelo para incluir la búsqueda semántica.
indexing-open-models = Abrir Modelos
indexing-embedding-model = Modelo de embeddings
indexing-hybrid = Búsqueda híbrida
indexing-none = Ninguno
indexing-keyword-only-badge = Solo palabras clave
indexing-embed-pending = Generar embeddings pendientes
indexing-retry-failed = Reintentar los fallos
indexing-reindex-all = Reindexar todo…
indexing-reindex-title = ¿Reindexar todos los documentos?
indexing-reindex-description = Los vectores de { $documents } ({ $chunks }) se generarán de nuevo con el modelo activo. La búsqueda sigue funcionando mientras tanto.
indexing-reindex-confirm = Reindexar
indexing-cancel = Cancelar
indexing-attention-heading = Requiere atención
indexing-recent-heading = Completados recientemente
indexing-retry = Intentar de nuevo
indexing-disabled-while-running = Disponible cuando termine la indexación en curso
indexing-stat-documents = Documentos
indexing-stat-chunks = Fragmentos
indexing-stat-dimensions = Dimensiones de los vectores
indexing-chunks-count =
    { $count ->
        [one] { $count } fragmento
       *[other] { $count } fragmentos
    }
indexing-attempts-count =
    { $count ->
        [one] { $count } intento
       *[other] { $count } intentos
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
indexing-docs-reading = { $count } en lectura
indexing-docs-awaiting = { $count } esperando embeddings
indexing-docs-no-text = { $count } sin texto
indexing-docs-failed = { $count } con error
indexing-chunks-total = { $count } en total
indexing-chunks-embedded = { $count } con vectores
indexing-chunks-pending = { $count } en espera
indexing-detail-waiting-model = Esperando un modelo de embeddings para entrar en la búsqueda semántica.
indexing-detail-no-embeddings = Los embeddings aún no se han generado.
indexing-detail-needs-ocr = Ninguna página tiene texto seleccionable. El reconocimiento de texto (OCR) aún no está disponible.
indexing-duration-under-second = menos de 1 s
indexing-duration-seconds = { $seconds } s
indexing-duration-minutes = { $minutes } min { $seconds } s
