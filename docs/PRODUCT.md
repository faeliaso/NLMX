# NLMX — Produto

Visão, requisitos e o que está feito. Como o sistema é construído: [`ARCHITECTURE.md`](ARCHITECTURE.md). Estado da última release: [`releases/0.1.0.md`](releases/0.1.0.md).

## Visão

**NLMX** é um app nativo para macOS que transforma uma coleção de PDFs numa base de conhecimento consultável em linguagem natural, com tudo rodando no próprio Mac. Documentos, índices e perguntas nunca saem do dispositivo.

- **Usuário-alvo:** profissionais que lidam com muitos PDFs (normas, contratos, manuais, artigos, documentação técnica) e não podem ou não querem enviar esse conteúdo para a nuvem.
- **Proposta de valor:** perguntar e receber uma resposta fundamentada, com **citações clicáveis** que levam à página e ao trecho exato do PDF.
- **Princípios:**
  1. Local-first de verdade — funciona offline após o download do modelo de embeddings.
  2. Zero atrito de instalação — um `.dmg`, sem Python, Ollama, Docker ou terminal.
  3. Confiança verificável — toda resposta mostra de onde veio; o app admite quando não encontrou.
  4. Nativo do macOS — usa o modelo do sistema (Apple Foundation Models) em vez de embarcar um LLM gerador.

## Restrições

- **macOS 27 ou superior, Apple Silicon.** Sem Intel, Rosetta, Windows ou Linux.
- Distribuição direta: `.dmg` assinado (Developer ID) e notarizado. Mac App Store fora de escopo (o sandbox impediria executar `/usr/bin/fm`).
- Geração só pelo `/usr/bin/fm` do sistema; **sem código Swift** (ADR 0002). Exige Apple Intelligence ativo e o aceite único de `sudo fm license`, que o app nunca faz pelo usuário.
- Sem Python, Ollama, Node ou servidor remoto no runtime. Única rede: download de modelos iniciado pelo usuário.

## Requisitos funcionais

✓ feito · ◐ parcial · ✗ não feito

| ID | Requisito | Status |
|---|---|---|
| **Biblioteca** | | |
| RF01 | Importar PDFs via seletor, drag & drop ou pasta (recursiva) | ◐ só seletor (vários arquivos) |
| RF02 | Detectar duplicatas por SHA-256 do conteúdo | ✓ |
| RF03 | Copiar o PDF para a biblioteca interna (`<data>/library/<sha>.pdf`) | ✓ |
| RF04 | Listar documentos com título, páginas, tamanho, data e status | ✓ |
| RF05 | Remover documento e todos os dados derivados | ✗ (cascatas prontas no banco, sem ação na UI) |
| RF06 | Reindexar (ex.: troca do modelo de embeddings) | ◐ automático ao trocar o modelo; sem ação manual |
| **Extração e estrutura** | | |
| RF07 | Texto por página via PDFium, com bounding box | ✓ |
| RF08 | Metadados (título, autor, datas) e outline | ◐ sem idioma nem outline; `ModDate` raramente vem do pdfium-render |
| RF09 | Normalização: hifenização, ligaduras, cabeçalhos/rodapés repetidos | ✓ |
| RF10 | Estrutura: títulos/seções, parágrafos, listas | ✓ |
| RF11 | Detectar PDFs escaneados (OCR fora de escopo por ora) | ✓ detecta (`needs_ocr`) |
| RF12 | Chunking por seção com sobreposição e página/bbox de origem | ✓ |
| **Embeddings e índices** | | |
| RF13 | Embeddings locais com llama.cpp (Metal) e GGUF multilíngue | ✓ `llama-server` + Qwen3-Embedding-0.6B |
| RF14 | Vetores em sqlite-vec e texto em FTS5, no mesmo SQLite | ✓ |
| RF15 | Ingestão em background retomável após reinício | ◐ retoma no boot; sem pausa/cancelamento |
| RF16 | Registrar modelo/versão de cada vetor; nunca misturar espaços | ✓ |
| **Busca e perguntas** | | |
| RF17 | Busca semântica, lexical e híbrida | ✓ fusão ponderada (ADR 0007) |
| RF18 | Escopo: biblioteca, documento ou coleção | ◐ documento por conversa; coleções só no backend |
| RF19 | Resposta via Apple FM com streaming | ✓ |
| RF20 | Citações `[n]` vinculadas a documento, página e trecho | ✓ (também `[página N]`) |
| RF21 | Clicar na citação abre o PDF na página com o trecho destacado | ✓ |
| RF22 | "Não encontrei" quando a relevância é baixa, sem chamar o modelo | ✓ |
| RF23 | Conversas salvas, com perguntas de acompanhamento | ✓ |
| RF24 | Transparência: mostrar os trechos enviados ao modelo | ✓ lista de fontes na resposta |
| **Modelos** | | |
| RF25 | Catálogo embarcado de modelos de embedding | ✓ |
| RF26 | Download com confirmação, progresso, retomada (Range), SHA-256 e checagem de disco | ✓ |
| RF27 | Importação manual de arquivo GGUF (instalação offline) | ✗ |
| RF28 | Detectar Apple FM e licença do `fm`, explicando como resolver | ✓ |

Também feito, fora da lista original: intenções "Explique este documento." e "seção N", regenerar/cancelar resposta, viewer com busca, camada de texto, zoom e miniaturas, diagnóstico local (métricas e logs sem conteúdo), `--self-check`.

## Requisitos não funcionais

| Categoria | Requisito | Medido (0.1.0, M4 16 GB) |
|---|---|---|
| Privacidade | Nenhum dado de documento sai do dispositivo; sem telemetria | `lsof`: só loopback e o socket do `fm`; canário de privacidade nos testes |
| Segurança | Sem servidor HTTP em porta TCP (exceção: `llama-server` em 127.0.0.1 com chave, ADR 0006); CSP restritiva; capabilities mínimas | ✓ |
| Ingestão | PDF de texto de 200 páginas indexado em < 60 s | ~16 s (p95, com embeddings) |
| Busca | Híbrida < 300 ms | p50 32 ms · p95 41 ms |
| Resposta | 1º token < 3 s | p50 0,7 s |
| Responsividade | UI nunca bloqueia durante a ingestão | ✓ |
| Robustez | PDF corrompido/com senha falha isolado, com mensagem | ✓ |
| Integridade | SQLite WAL, migrações reversíveis, gravação transacional por documento | ✓ |
| Dados | Tudo em `~/Library/Application Support/dev.nlmx.desktop/` | ✓ |
| Idiomas | UI em pt-BR; pergunta em PT encontra trecho em EN | ✓ |
| Observabilidade | Logs JSON locais com rotação, sem conteúdo | ✓ ([`TESTING.md`](TESTING.md#logs-estruturados)) |

## Riscos em aberto

| Risco | Mitigação atual |
|---|---|
| Janela do Apple FM (4 096 tokens) limita perguntas amplas | Orçamento de ~1 800 tokens de contexto, contagem exata com `fm count-tokens` |
| Guardrails do FM recusam conteúdo legítimo | Recusa mostrada com os trechos recuperados |
| Interface do `fm` muda com atualizações do macOS | Status classificado, fallback `fm respond`, testes de contrato |
| Qualidade de extração (colunas, tabelas, escaneados) | Fixtures + conjunto-ouro; OCR pendente |
| sqlite-vec pré-1.0, força bruta | Adequado a ~100k chunks; port `VectorStore` permite trocar |
| FTS5 sem stemming para português | Remoção de diacríticos; o vetor cobre a semântica |

## Próximos passos

1. **Distribuição pública:** certificado Developer ID e `make release` (assinatura, notarização), depois `make acceptance` e teste num Mac limpo.
2. **Lacunas de requisitos:** remover documento (RF05), drag & drop e pasta (RF01), importação manual de GGUF (RF27), coleções na UI (RF18), pausa/cancelamento da ingestão (RF15).
3. **Desempenho dos embeddings:** reduzir contexto/lote do `llama-server` (~1,9 GB e 8,4 embeddings/s hoje). Medir com `make bench`.
4. **OCR** de escaneados (ex.: `fm respond --tool ocr`), preservando página e coordenadas.
5. **Qualidade:** títulos numerados repetidos removidos como cabeçalho; documentos padronizados que só mudam números.
6. **Primeiro uso guiado:** checklist na primeira abertura (licença do `fm`, Apple Intelligence, modelo).

**Fora de escopo:** Intel/Windows/Linux, LLM gerador alternativo, reranker, sincronização, anotação de PDFs, Mac App Store.
