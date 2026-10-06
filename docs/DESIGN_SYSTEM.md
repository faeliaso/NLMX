# NLMX — Design System

Modern, minimalist macOS interface: lots of white space, system typography, soft borders, subtle shadows with a hairline, blue as the action color, automatic light/dark and WCAG 2.2 AA accessibility.

Interactive gallery (debug builds only): **Configurações › Sobre › Design System** (Settings › About › Design System), or `NLMX_START_PATH="/design-system?theme=dark" make dev`.

## Where everything lives

| File | Contents |
|---|---|
| `apps/desktop/ui/styles/tokens.css` | **Single source** of colors, typography, radius, shadows, spacing, control heights and motion |
| `apps/desktop/ui/styles/base.css` | Base: body, focus, selection, scrollbars, `sr-only`, reduced motion, forced colors |
| `apps/desktop/ui/styles/components/*.css` | Components as semantic classes (`.btn`, `.field`, `.card`, `.badge`, `.menu`, `.dialog`, `.progress`, `.skeleton`, `.alert`, `.empty`, `.toast`) |
| `apps/desktop/ui/styles/shell.css` | App layout (sidebar, header, status bar, composer, segmented) |
| `apps/desktop/ui/components/ds.html` | askama macros with the accessible markup of the components |
| `apps/desktop/ui/components/icons/*.html` | SVG icons (stroke 1.6, `currentColor`, `aria-hidden`) |
| `apps/desktop/ui/scripts/ds.js` | Behavior: theme, dialog, menu (keyboard), toast, dismiss |

## Rules

1. **No raw colors outside `tokens.css`.** `@theme` removes Tailwind's default palettes (`--color-*: initial`); only utilities from the tokens exist (`bg-surface`, `text-fg-muted`, `border-border`…).
2. **Do not use `dark:`.** Every color is `light-dark(LIGHT, DARK)` and follows `color-scheme`, which follows the system or `<html data-theme="light|dark">`.
3. **Contrast is checked by hand.** `ui-web` no longer has automated tests (neither the contrast one nor the one that blocked raw colors): when changing `tokens.css`, check ≥ 4.5:1 for text and ≥ 3:1 for control outlines in both themes.
4. **Color is never the only signal:** badges and alerts always have text; errors have an icon + message.
5. **Focus is always visible** (`:focus-visible`, `--color-focus` ring); targets ≥ 24px; animations respect `prefers-reduced-motion`.
6. **Page header on one line.** Title and description (`.page-title`, `.page-description`) end in an ellipsis when the window narrows, and the full text goes in the `title` attribute; use the `page::header` macro instead of building the header by hand.

## Tokens

**Semantic colors** — surfaces (`bg`, `bg-subtle`, `surface`, `surface-raised`, `surface-hover`, `surface-pressed`), text (`fg`, `fg-muted`, `fg-subtle` only for placeholder/disabled), lines (`border` decorative, `border-strong` for controls), action (`accent` fill, `accent-text` links, `accent-soft` selection, `accent-fg` text on accent), `success|warning|danger|info` states with three variants each: base (indicators), `-fg` (accessible text), `-soft` (background).

**Typography** (system font / SF): `caption` 11 · `footnote` 12 · `body` 13 (macOS default) · `callout` 14 · `title-3` 15 · `title-2` 17 · `title-1` 22 · `large-title` 26. Weights 400/500/600.

**Spacing** — Tailwind's 4px grid (`p-4` = 16px) + aliases `--space-control-x` 10px, `--space-card` 16px, `--space-section` 32px, `--space-page` 40px.

**Radius** — `xs` 4 (badge, checkbox) · `sm` 6 (button, input) · `md` 8 (menu) · `lg` 10 (card) · `xl` 14 (dialog) · `full`.

**Shadows** — `xs` controls · `sm` cards · `md` menus/toasts · `lg` dialogs; always with a 0.5px hairline.

**Controls** — heights `--control-sm` 24 · `--control-md` 28 · `--control-lg` 36.

## Components

| Component | Usage |
|---|---|
| Button | `.btn` + `.btn-primary` / `.btn-secondary` / `.btn-ghost` / `.btn-destructive`; sizes `.btn-sm` / `.btn-lg`; `.btn-icon` (requires `aria-label`). Loading: `aria-busy="true"` or automatic during an HTMX request (`.htmx-request`). |
| Field | `{% call ds::field(id, label, help, error, required) %}<input class="input" id=… aria-describedby="ID-help ID-error">{% endcall %}`; `aria-invalid="true"` when there is an error. Also `.select`, `.textarea`, `.search`, `.check` (checkbox/radio), `.check.switch`. |
| Card / list | `.card` + `.card-header` / `.card-body` / `.card-footer`; `.card-interactive`; `.list` + `.list-row`. |
| Badge | `{% call ds::badge(text, kind, dot) %}` — kind: neutral, accent, success, warning, danger, info. |
| Menu | Trigger `popovertarget="ID" aria-haspopup="menu"` + `<div class="menu" id="ID" popover role="menu">` with `ds::menu_item`. Arrows ↑↓, Home/End, Esc. |
| Dialog | `{% call ds::dialog(id, title, description, destructive) %}…buttons…{% endcall %}`; open with `data-dialog-open="ID"`, close with `data-dialog-close`. Focus is trapped and returned to the trigger. Icon and title centered (`.dialog-confirm`); description and buttons are not. |
| Progress | `ds::progress(value, label, kind)`, `ds::progress_indeterminate(label)`, `ds::spinner(label, size)`. |
| Skeleton | `.skeleton` + `-line` / `-title` / `-block` / `-circle`; `ds::skeleton_card()`. Container with `aria-busy="true"` and `sr-only` text. |

## States

| State | Pattern |
|---|---|
| Loading | Skeleton in place of the content; spinner on actions; global bar at the top of the content during navigation |
| Source (any format) | Format icon (`format-pdf/markdown/text/csv/epub/docx/xlsx (same family: page outline + glyph)`, coming from `ui-web::formats`, never a `match` in the view) + name + format + status + location/meta. Only PDF has a preview (viewer); clicking any source or `[n]` reference opens the side panel: viewer (PDF) or **Informações da fonte** (Source information) (`/sources/{id}`, the other formats). A `[n]` is always a button (`aria-label` says "Abrir a fonte n no PDF" (Open source n in the PDF) or "Ver informações da fonte n" (View source n information)). |
| Empty | `ds::empty_state(icon, title, description)` with the primary action in the body |
| Error | `ds::alert("danger", …)` (`role="alert"`) with a recovery action; failures with no response become an error toast |
| Success | `ds::alert("success", …)` or `DS.toast("success", msg)` (`role="status"`, disappears after 5 s). A server response requests the toast with a marker `<p hidden data-toast-on-load="KIND" data-toast-message="…"></p>` (`app.js` turns it into a toast and removes it) |
