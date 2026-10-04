# NLMX — Design System

Interface macOS moderna e minimalista: muito espaço em branco, tipografia do sistema, bordas suaves, sombras sutis com hairline, azul como cor de ação, light/dark automáticos e acessibilidade WCAG 2.2 AA.

Galeria interativa (somente builds debug): **Configurações › Sobre › Design System**, ou `NLMX_START_PATH="/design-system?theme=dark" make dev`.

## Onde fica cada coisa

| Arquivo | Conteúdo |
|---|---|
| `apps/desktop/ui/styles/tokens.css` | **Fonte única** de cores, tipografia, radius, sombras, espaçamento, alturas de controle e movimento |
| `apps/desktop/ui/styles/base.css` | Base: corpo, foco, seleção, scrollbars, `sr-only`, reduced motion, forced colors |
| `apps/desktop/ui/styles/components/*.css` | Componentes como classes semânticas (`.btn`, `.field`, `.card`, `.badge`, `.menu`, `.dialog`, `.progress`, `.skeleton`, `.alert`, `.empty`, `.toast`) |
| `apps/desktop/ui/styles/shell.css` | Layout do app (sidebar, header, status bar, composer, segmented) |
| `apps/desktop/ui/components/ds.html` | Macros askama com a marcação acessível dos componentes |
| `apps/desktop/ui/components/icons/*.html` | Ícones SVG (stroke 1.6, `currentColor`, `aria-hidden`) |
| `apps/desktop/ui/scripts/ds.js` | Comportamento: tema, dialog, menu (teclado), toast, dismiss |

## Regras

1. **Sem cores cruas fora de `tokens.css`.** O `@theme` remove as paletas padrão do Tailwind (`--color-*: initial`); só existem utilities dos tokens (`bg-surface`, `text-fg-muted`, `border-border`…).
2. **Não use `dark:`.** Cada cor é `light-dark(CLARO, ESCURO)` e acompanha `color-scheme`, que segue o sistema ou `<html data-theme="light|dark">`.
3. **Contraste é testado.** `crates/ui-web/tests/contrast.rs` lê `tokens.css` e exige ≥ 4.5:1 para texto e ≥ 3:1 para contornos de controles nos dois temas. Novo par texto/fundo → adicione em `PAIRS`.
4. **Cor nunca é o único sinal:** badges e alerts sempre têm texto; erros têm ícone + mensagem.
5. **Foco visível sempre** (`:focus-visible`, anel `--color-focus`); alvos ≥ 24px; animações respeitam `prefers-reduced-motion`.

## Tokens

**Cores semânticas** — superfícies (`bg`, `bg-subtle`, `surface`, `surface-raised`, `surface-hover`, `surface-pressed`), texto (`fg`, `fg-muted`, `fg-subtle` só para placeholder/desabilitado), linhas (`border` decorativa, `border-strong` para controles), ação (`accent` preenchimento, `accent-text` links, `accent-soft` seleção, `accent-fg` texto sobre accent), estados `success|warning|danger|info` com três variantes cada: base (indicadores), `-fg` (texto acessível), `-soft` (fundo).

**Tipografia** (fonte do sistema / SF): `caption` 11 · `footnote` 12 · `body` 13 (padrão macOS) · `callout` 14 · `title-3` 15 · `title-2` 17 · `title-1` 22 · `large-title` 26. Pesos 400/500/600.

**Espaçamento** — grade de 4px do Tailwind (`p-4` = 16px) + aliases `--space-control-x` 10px, `--space-card` 16px, `--space-section` 32px, `--space-page` 40px.

**Radius** — `xs` 4 (badge, checkbox) · `sm` 6 (botão, input) · `md` 8 (menu) · `lg` 10 (card) · `xl` 14 (dialog) · `full`.

**Sombras** — `xs` controles · `sm` cards · `md` menus/toasts · `lg` dialogs; sempre com hairline de 0.5px.

**Controles** — alturas `--control-sm` 24 · `--control-md` 28 · `--control-lg` 36.

## Componentes

| Componente | Uso |
|---|---|
| Botão | `.btn` + `.btn-primary` / `.btn-secondary` / `.btn-ghost` / `.btn-destructive`; tamanhos `.btn-sm` / `.btn-lg`; `.btn-icon` (exige `aria-label`). Loading: `aria-busy="true"` ou automático durante request HTMX (`.htmx-request`). |
| Campo | `{% call ds::field(id, label, help, error, required) %}<input class="input" id=… aria-describedby="ID-help ID-error">{% endcall %}`; `aria-invalid="true"` quando há erro. Também `.select`, `.textarea`, `.search`, `.check` (checkbox/radio), `.check.switch`. |
| Card / lista | `.card` + `.card-header` / `.card-body` / `.card-footer`; `.card-interactive`; `.list` + `.list-row`. |
| Badge | `{% call ds::badge(texto, kind, dot) %}` — kind: neutral, accent, success, warning, danger, info. |
| Menu | Gatilho `popovertarget="ID" aria-haspopup="menu"` + `<div class="menu" id="ID" popover role="menu">` com `ds::menu_item`. Setas ↑↓, Home/End, Esc. |
| Dialog | `{% call ds::dialog(id, título, descrição, destructive) %}…botões…{% endcall %}`; abrir com `data-dialog-open="ID"`, fechar com `data-dialog-close`. Foco preso e devolvido ao gatilho. Ícone e título centralizados (`.dialog-confirm`); descrição e botões não. |
| Progresso | `ds::progress(valor, rótulo, kind)`, `ds::progress_indeterminate(rótulo)`, `ds::spinner(rótulo, tamanho)`. |
| Skeleton | `.skeleton` + `-line` / `-title` / `-block` / `-circle`; `ds::skeleton_card()`. Contêiner com `aria-busy="true"` e texto `sr-only`. |

## Estados

| Estado | Padrão |
|---|---|
| Loading | Skeleton no lugar do conteúdo; spinner em ações; barra global no topo do conteúdo durante navegação |
| Empty | `ds::empty_state(ícone, título, descrição)` com a ação principal no corpo |
| Error | `ds::alert("danger", …)` (`role="alert"`) com ação de recuperação; falhas sem resposta viram toast de erro |
| Success | `ds::alert("success", …)` ou `DS.toast("success", msg)` (`role="status"`, some em 5 s) |
