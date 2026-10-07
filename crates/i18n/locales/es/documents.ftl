documents-title = Documentos
documents-description = Documentos de tu biblioteca local. Nada sale de este Mac.
documents-import = Importar documentos
documents-add-note = Añadir nota
documents-library-unavailable = Biblioteca no disponible
documents-empty-title = Ningún documento
documents-empty-description = Usa Importar documentos, en la parte superior de la página, para empezar (PDF, Markdown, TXT, CSV/TSV, EPUB, DOCX o XLSX), o Añadir nota para pegar un texto. Los archivos se copian a la biblioteca local y se dividen en fragmentos para la búsqueda.
documents-note-dialog-title = Pega el texto copiado
documents-note-dialog-description = Pega el texto copiado a continuación para añadirlo como una fuente.
documents-note-placeholder = Pega o escribe aquí el texto de la nota
documents-note-label = Texto de la nota
documents-note-insert = Insertar
documents-note-name = Nota
documents-open = Abrir
documents-details = Detalles
documents-actions = Acciones
documents-more-actions = Más acciones para { $title }
documents-more-actions-disabled-title = Disponible cuando termine la lectura del documento
documents-more-actions-disabled-label = Más acciones (no disponible mientras se procesa el documento)
documents-remove-menu = Eliminar…
documents-remove = Eliminar
documents-remove-dialog-title = ¿Eliminar “{ $title }”?
documents-pages =
    { $count ->
        [one] { $count } página
       *[other] { $count } páginas
    }
documents-chunks =
    { $count ->
        [one] { $count } fragmento
       *[other] { $count } fragmentos
    }
documents-date = { $day }/{ $month }/{ $year }
documents-status-embedding = Esperando embeddings
documents-status-indexed = Indexado
documents-status-needs-ocr = Sin texto (OCR)
documents-status-failed = Falló
documents-status-queued = En cola
documents-status-extracting = Leyendo el archivo
documents-status-structuring = Estructurando
documents-status-chunking = Dividiendo en fragmentos
documents-removal-base =
    { $chunks ->
        [0] La copia del documento en la biblioteca se borrará de este Mac.
        [1] La copia del documento en la biblioteca, su único fragmento y los índices de búsqueda se borrarán de este Mac.
       *[other] La copia del documento en la biblioteca, sus { $chunks } fragmentos y los índices de búsqueda se borrarán de este Mac.
    }
documents-removal-conversations =
    { $count ->
        [one] se eliminará { $count } conversación
       *[other] se eliminarán { $count } conversaciones
    }
documents-removal-turns =
    { $count ->
        [one] se quitará { $count } par de pregunta y respuesta que lo usó
       *[other] se quitarán { $count } pares de pregunta y respuesta que lo usaron
    }
documents-removal-history = En el Chat, { $items }.
documents-removal-join = { $first } y { $second }
documents-removal-original = El archivo original no se ve afectado.
documents-notice-removed = Documento eliminado. Él y todo lo que derivaba de él se borraron de este Mac.
documents-notice-remove-failed = No se pudo eliminar el documento: { $reason }
documents-notice-note-unavailable = No se pudo añadir la nota: la importación no está disponible.
documents-notice-note-added = Nota añadida. Se indexa en segundo plano.
documents-picker-title = Importar documentos
documents-picker-filter = Documentos
documents-import-started =
    { $count ->
        [one] Importando { $count } archivo…
       *[other] Importando { $count } archivos…
    }
documents-import-imported =
    { $count ->
        [one] { $count } importado
       *[other] { $count } importados
    }
documents-import-duplicates =
    { $count ->
        [one] { $count } ya estaba en la biblioteca
       *[other] { $count } ya estaban en la biblioteca
    }
documents-import-failed = { $count } con errores
documents-import-summary-failure = { $summary } — { $first }
documents-import-internal-register = fallo interno al registrar el archivo
documents-import-internal-process = fallo interno al procesar el archivo
documents-import-internal-note = fallo interno al registrar la nota
