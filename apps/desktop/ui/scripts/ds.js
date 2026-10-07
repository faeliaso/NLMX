// NLMX design-system behaviour: theme, dialogs, menus, toasts.
// Loaded in <head> without `defer` so the stored theme applies before first paint.
(() => {
  "use strict";

  const THEME_KEY = "nlmx.theme";
  const root = document.documentElement;

  const storage = {
    get(key) { try { return localStorage.getItem(key); } catch { return null; } },
    set(key, value) { try { localStorage.setItem(key, value); } catch { /* storage unavailable */ } },
  };

  // ── Interface strings: `<script type="application/json" id="i18n-js">` (the `js-*` catalog ids
  // of the active language, rendered by the server). Variables are `{name}` placeholders.
  let strings = null;
  function nlmxT(key, args) {
    if (!strings) {
      const node = document.getElementById("i18n-js");
      if (!node) return "";
      try { strings = JSON.parse(node.textContent || "{}"); } catch { strings = {}; }
    }
    let text = strings[key] ?? "";
    if (args) for (const [name, value] of Object.entries(args)) text = text.split(`{${name}}`).join(String(value));
    return text;
  }
  nlmxT.reload = () => { strings = null; };
  window.nlmxT = nlmxT;

  // ── Theme: "system" | "light" | "dark" ──────────────────────────────────
  function applyTheme(theme) {
    if (theme === "light" || theme === "dark") root.dataset.theme = theme;
    else delete root.dataset.theme;
    document.querySelectorAll("[data-theme-set]").forEach((el) => {
      el.setAttribute("aria-pressed", String(el.dataset.themeSet === (theme || "system")));
    });
  }
  function setTheme(theme) { storage.set(THEME_KEY, theme); applyTheme(theme); }

  // A server-forced theme (data-theme already on <html>) wins over the stored one.
  if (!root.dataset.theme) applyTheme(storage.get(THEME_KEY) || "system");

  // ── Toasts (markup comes from <template id="toast-template-KIND"> in the layout) ──
  function toast(kind, message, { timeout } = {}) {
    const region = document.getElementById("toast-region");
    if (!region) return;
    const template = document.getElementById(`toast-template-${kind}`) || document.getElementById("toast-template-info");
    const node = template.content.firstElementChild.cloneNode(true);
    node.querySelector("[data-toast-message]").textContent = message;
    region.append(node);
    const ms = timeout ?? (kind === "danger" ? 0 : 5000);
    if (ms > 0) setTimeout(() => node.remove(), ms);
    return node;
  }

  // ── Menus (popover API + keyboard) ──────────────────────────────────────
  function menuItems(menu) {
    return [...menu.querySelectorAll('[role="menuitem"]:not([aria-disabled="true"])')];
  }
  function positionMenu(menu) {
    const invoker = document.querySelector(`[popovertarget="${menu.id}"]`);
    if (!invoker) return;
    const r = invoker.getBoundingClientRect();
    const width = menu.offsetWidth;
    const left = Math.min(r.left, window.innerWidth - width - 8);
    menu.style.left = `${Math.max(8, left)}px`;
    menu.style.top = `${r.bottom + 4}px`;
  }

  document.addEventListener("toggle", (event) => {
    const menu = event.target;
    if (!(menu instanceof HTMLElement) || !menu.matches(".menu")) return;
    const invoker = document.querySelector(`[popovertarget="${menu.id}"]`);
    invoker?.setAttribute("aria-expanded", String(event.newState === "open"));
    if (event.newState === "open") {
      positionMenu(menu);
      menuItems(menu)[0]?.focus();
    }
  }, true);

  document.addEventListener("keydown", (event) => {
    const menu = event.target.closest?.(".menu");
    if (!menu) return;
    const items = menuItems(menu);
    const index = items.indexOf(document.activeElement);
    const move = { ArrowDown: index + 1, ArrowUp: index - 1, Home: 0, End: items.length - 1 }[event.key];
    if (move !== undefined) {
      event.preventDefault();
      items[(move + items.length) % items.length]?.focus();
    }
  });

  // ── Delegated clicks: theme, dialogs, menu items, dismiss ───────────────
  let dialogOpener = null;
  document.addEventListener("click", (event) => {
    const target = event.target.closest?.("[data-theme-set], [data-dialog-open], [data-dialog-close], [role='menuitem'], [data-dismiss], [data-toast]");
    if (!target) return;

    if (target.dataset.themeSet) setTheme(target.dataset.themeSet);

    if (target.dataset.dialogOpen) {
      const dialog = document.getElementById(target.dataset.dialogOpen);
      dialogOpener = target;
      dialog?.showModal();
    }
    if (target.hasAttribute("data-dialog-close")) target.closest("dialog")?.close(target.dataset.dialogClose || "");

    if (target.getAttribute("role") === "menuitem") target.closest(".menu")?.hidePopover();

    if (target.hasAttribute("data-dismiss")) target.closest("[data-dismissible]")?.remove();

    if (target.dataset.toast) toast(target.dataset.toast, target.dataset.toastMessage || "");
  });

  document.addEventListener("close", (event) => {
    if (event.target instanceof HTMLDialogElement && dialogOpener) {
      dialogOpener.focus();
      dialogOpener = null;
    }
  }, true);

  // Keep theme controls in sync on first load and whenever HTMX swaps new content in.
  const syncTheme = () => applyTheme(root.dataset.theme || storage.get(THEME_KEY) || "system");
  document.addEventListener("DOMContentLoaded", syncTheme);
  document.addEventListener("htmx:after:settle", syncTheme);

  window.DS = { setTheme, toast };
})();
