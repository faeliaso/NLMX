# ADR 0016 — Fontes, preview e citação na interface

- Status: aceito
- Data: 2026-10-04
- Atende: a interface trata PDF, Markdown, TXT, CSV e EPUB como fontes equivalentes (continua o ADR 0015). **Substitui** a decisão de que citações de formatos não-PDF não são clicáveis.

## Contexto
O RAG já devolve proveniência de qualquer formato, mas a interface mostrava um ícone único, fontes não-PDF como itens inertes e não tinha onde dizer o que uma fonte é.

## Decisão
- **Três coisas separadas:** *Source* (todo documento: ícone, nome, formato, status, informação), *Preview* (só o PDF, no visualizador existente) e *citação do RAG* (liga as duas).
- **Painel lateral único.** O slot `#viewer` do chat recebe o viewer (PDF) ou **Informações da fonte** (`GET /sources/{id}[?cite=…]`, `ui-web/src/sources.rs`): formato, status, nº de trechos, tamanho, importado/indexado em, "usado em N conversas" (`DocumentRepository::source_details`), o trecho citado quando veio de uma resposta e, para PDF, o atalho "Abrir no PDF". Não há visualizador genérico e o conteúdo de Markdown/TXT/CSV/EPUB nunca é exibido. Abertura, fechamento, Esc, foco e modo estreito são os do viewer.
- **Clique.** Em PDF abre o viewer na página; em qualquer outro formato seleciona a fonte e mostra as informações. `[n]` é sempre um botão, com `aria-label` que diz o destino.
- **Um lugar para formatos.** `ui-web/src/formats.rs` mapeia `DocumentType` → ícone e rótulo; Documentos ("Detalhes" para não-PDF, `/chat?source=`), chat, Indexação e painel o usam. O script do progresso de indexação não conhece formatos: clona ícones de um `<template>` renderizado pelo servidor.
- **Stack.** HTMX 4 + askama + Tailwind 4 + tokens e JS puro; sem TypeScript/Node (CLAUDE.md) nem dependência de Lucide: os ícones novos são SVGs no estilo Lucide em `ui/components/icons`.

## Consequências
- Um `<select>` nativo não tem ícones: o seletor de escopo do chat mostra o formato em texto.
- O número de conversas vem de citações marcadas como citadas; fontes só consultadas não contam.
