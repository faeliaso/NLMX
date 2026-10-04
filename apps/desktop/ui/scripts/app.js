// NLMX application shell: global loading, error handling, Tauri commands, shortcuts.
(() => {
  "use strict";

  const root = document.documentElement;
  const invoke = window.__TAURI__?.core?.invoke;

  // ── Global loading: html[data-busy] while any HTMX request is in flight ─────
  // Shown after a short delay so fast responses don't flash the indicator.
  let inFlight = 0;
  let busyTimer = null;
  function updateBusy() {
    clearTimeout(busyTimer);
    if (inFlight > 0) busyTimer = setTimeout(() => { root.dataset.busy = ""; }, 150);
    else delete root.dataset.busy;
  }
  document.addEventListener("htmx:before:request", (event) => {
    if (event.target.closest?.("#system-status, [data-poll]")) return; // background polling stays silent
    inFlight += 1;
    updateBusy();
  });
  document.addEventListener("htmx:finally:request", (event) => {
    if (event.target.closest?.("#system-status, [data-poll]")) return;
    inFlight = Math.max(0, inFlight - 1);
    updateBusy();
  });

  // ── Server-requested toasts: <p hidden data-toast-on-load="KIND" data-toast-message="…"> ──
  // A response that wants to announce an outcome (e.g. a document removed) carries this marker;
  // it becomes a toast and is removed so it never shows twice. (HTMX 4 has no HX-Trigger header.)
  function showMarkedToasts(scope) {
    const markers = scope.matches?.("[data-toast-on-load]") ? [scope] : [];
    markers.push(...(scope.querySelectorAll?.("[data-toast-on-load]") ?? []));
    for (const marker of markers) {
      window.DS?.toast(marker.dataset.toastOnLoad || "info", marker.dataset.toastMessage || "");
      marker.remove();
    }
  }
  new MutationObserver((mutations) => {
    for (const m of mutations) m.addedNodes.forEach((node) => node.nodeType === 1 && showMarkedToasts(node));
  }).observe(document.body, { childList: true, subtree: true });
  document.addEventListener("DOMContentLoaded", () => showMarkedToasts(document.body));

  // ── Errors ──────────────────────────────────────────────────────────────────
  // HTTP errors arrive as rendered error fragments (HTMX 4 swaps 4xx/5xx). Network or
  // protocol failures have no response to swap, so they surface as a toast.
  document.addEventListener("htmx:error", () => {
    window.DS?.toast("danger", "Não foi possível carregar o conteúdo. Tente novamente.");
  });

  function reportError(message, source) {
    invoke?.("report_client_error", { message: String(message), source: String(source || "") }).catch(() => {});
  }
  window.addEventListener("error", (event) => {
    reportError(event.message, `${event.filename}:${event.lineno}`);
    window.DS?.toast("danger", "Ocorreu um erro inesperado na interface.");
  });
  window.addEventListener("unhandledrejection", (event) => {
    reportError(event.reason?.message || event.reason, "unhandledrejection");
  });

  // ── Tauri commands: <button data-command="name"> ────────────────────────────
  document.addEventListener("click", async (event) => {
    const button = event.target.closest?.("[data-command]");
    if (!button) return;
    if (!invoke) {
      window.DS?.toast("danger", "Comandos nativos indisponíveis fora do aplicativo.");
      return;
    }
    let args = {};
    try {
      args = button.dataset.commandArgs ? JSON.parse(button.dataset.commandArgs) : {};
    } catch {
      window.DS?.toast("danger", "Comando inválido.");
      return;
    }
    if (button.dataset.command === "download_model") showDownload(args.id);
    button.setAttribute("aria-busy", "true");
    try {
      // Commands may return { kind, message, refresh }: a toast and a DOM event that makes
      // HTMX reload the affected fragments (hx-trigger="<refresh> from:body").
      const outcome = await invoke(button.dataset.command, args);
      if (outcome?.message) window.DS?.toast(outcome.kind || "info", outcome.message);
      if (outcome?.refresh) document.body.dispatchEvent(new CustomEvent(outcome.refresh, { bubbles: true }));
    } catch (error) {
      window.DS?.toast("danger", error?.message || String(error));
    } finally {
      button.removeAttribute("aria-busy");
    }
  });

  // ── Background indexing: the app emits "indexing-changed" when work ends ─────
  // (startup resume, embeddings after a model change, Indexação actions).
  window.__TAURI__?.event?.listen("indexing-changed", () => {
    document.body.dispatchEvent(new CustomEvent("indexing-changed", { bubbles: true }));
  });

  // ── Imports run in the background: the library list follows them ──
  // The app emits "documents-changed" whenever a document is registered or changes stage (many
  // times while embedding); the list reloads at most once per short window.
  let documentsTimer = null;
  window.__TAURI__?.event?.listen("documents-changed", () => {
    if (documentsTimer) return;
    documentsTimer = setTimeout(() => {
      documentsTimer = null;
      document.body.dispatchEvent(new CustomEvent("documents-changed", { bubbles: true }));
    }, 300);
  });
  // One summary per batch of imported files; failures stay until dismissed.
  window.__TAURI__?.event?.listen("import-finished", ({ payload }) => {
    if (payload?.message) window.DS?.toast(payload.kind || "info", payload.message);
  });

  // ── Model downloads: progress bar driven by "model-download-progress" events ──
  const megabytes = (bytes) => `${(bytes / 1e6).toFixed(0)} MB`;
  function showDownload(id) {
    document.querySelector(`[data-download-progress="${id}"]`)?.removeAttribute("hidden");
    document.querySelector(`[data-download-cancel="${id}"]`)?.removeAttribute("hidden");
    document.querySelector(`[data-download-start="${id}"]`)?.setAttribute("hidden", "");
  }
  window.__TAURI__?.event?.listen("model-download-progress", ({ payload }) => {
    const box = document.querySelector(`[data-download-progress="${payload.id}"]`);
    if (!box) return;
    showDownload(payload.id);
    const percent = payload.total ? Math.floor((payload.received / payload.total) * 100) : 0;
    const bar = box.querySelector(".progress");
    bar?.setAttribute("aria-valuenow", String(percent));
    bar?.querySelector(".progress-bar")?.style.setProperty("--value", `${percent}%`);
    const speed = payload.bytes_per_second > 0 ? ` · ${megabytes(payload.bytes_per_second)}/s` : "";
    const label = box.querySelector("[data-download-label]");
    if (label) label.textContent = `${megabytes(payload.received)} de ${megabytes(payload.total)} (${percent}%)${speed}`;
  });

  // ── Chat ────────────────────────────────────────────────────────────────────
  // Questions are posted with HTMX (the server answers with the turn and a "streaming"
  // placeholder); the answer text then streams through a Tauri Channel (`answer_message`),
  // and when it is done the final fragment (citations, sources) replaces the placeholder.

  const chatForm = () => document.getElementById("chat-form");
  const chatLog = () => document.getElementById("chat-log");
  const streaming = () => document.querySelector('[data-answer][data-status="streaming"]');

  function autogrow(field) {
    field.style.height = "auto";
    field.style.height = `${field.scrollHeight}px`;
  }
  document.addEventListener("input", (event) => {
    if (event.target.matches?.("textarea[data-autogrow]")) autogrow(event.target);
  });

  // One answer at a time: while one is being generated the composer is locked and its
  // send button becomes a stop button (a `cancel_answer` command, see "Tauri commands").
  function updateComposer() {
    const form = chatForm();
    if (!form) return;
    const answer = streaming();
    form.toggleAttribute("data-locked", Boolean(answer));
    const button = form.querySelector(".composer-action");
    if (!button || form.querySelector("textarea")?.disabled) return;
    const label = answer ? "Parar resposta" : "Enviar pergunta";
    button.type = answer ? "button" : "submit";
    button.setAttribute("aria-label", label);
    button.title = label;
    if (answer) {
      button.dataset.command = "cancel_answer";
      button.dataset.commandArgs = JSON.stringify({ messageId: Number(answer.dataset.answer) });
    } else {
      delete button.dataset.command;
      delete button.dataset.commandArgs;
    }
  }

  // Enter sends (Shift+Enter = new line).
  document.addEventListener("keydown", (event) => {
    const field = event.target.closest?.("textarea[data-submit-on-enter]");
    if (field && event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      if (field.value.trim() && !field.form?.hasAttribute("data-locked")) field.form?.requestSubmit();
    }
  });

  // Suggestions fill the question field.
  document.addEventListener("click", (event) => {
    const suggestion = event.target.closest?.("[data-fill-question]");
    if (!suggestion) return;
    const field = document.getElementById("question");
    if (!field || field.disabled) return;
    field.value = suggestion.dataset.fillQuestion;
    autogrow(field);
    field.focus();
  });

  document.addEventListener("htmx:before:request", (event) => {
    if (event.target?.id !== "chat-form") return;
    if (event.target.hasAttribute("data-locked")) {
      event.preventDefault();
      return;
    }
    document.querySelector("[data-chat-empty]")?.remove();
    const field = document.getElementById("question");
    // The request already captured the value: clear the input for the next question.
    if (field) queueMicrotask(() => { field.value = ""; autogrow(field); });
  });

  function nearBottom(log) {
    return log.scrollHeight - log.scrollTop - log.clientHeight < 120;
  }
  function scrollToTurn(turn) {
    const log = chatLog();
    if (log && turn) log.scrollTo({ top: turn.offsetTop - 16, behavior: "smooth" });
  }

  // Replaces an answer with its current rendering (final once generated).
  function refreshAnswer(id) {
    const target = document.getElementById(`answer-${id}`);
    if (!target || !window.htmx) return;
    window.htmx.ajax("GET", `/chat/messages/${id}`, { target, swap: "outerHTML" });
  }

  function startAnswer(el) {
    el.dataset.started = "";
    const id = Number(el.dataset.answer);
    const Channel = window.__TAURI__?.core?.Channel;
    if (!invoke || !Channel) {
      window.DS?.toast("danger", "A geração de respostas só funciona dentro do aplicativo.");
      return;
    }
    const output = el.querySelector("[data-answer-stream]");
    const channel = new Channel();
    channel.onmessage = (event) => {
      if (event.kind === "token" && output) {
        const log = chatLog();
        const follow = log && nearBottom(log);
        output.textContent += event.text;
        el.dataset.streamingText = "";
        if (follow) log.scrollTop = log.scrollHeight;
      } else if (event.kind === "done") {
        refreshAnswer(id);
      }
    };
    invoke("answer_message", { messageId: id, onEvent: channel }).catch((error) => {
      if (error?.code === "running") {
        // Being generated elsewhere (e.g. before navigating away): check again shortly.
        setTimeout(() => refreshAnswer(id), 1500);
      } else {
        window.DS?.toast("danger", error?.message || String(error));
        refreshAnswer(id);
      }
    });
  }

  function startPending() {
    document
      .querySelectorAll('[data-answer][data-status="streaming"]:not([data-started])')
      .forEach(startAnswer);
    updateComposer();
  }
  document.addEventListener("htmx:after:settle", startPending);
  document.addEventListener("DOMContentLoaded", startPending);
  if (document.readyState !== "loading") startPending();

  // New turns scroll into view.
  new MutationObserver((mutations) => {
    for (const m of mutations) {
      for (const node of m.addedNodes) {
        if (node instanceof HTMLElement && node.matches("[data-chat-turn]")) scrollToTurn(node);
      }
    }
  }).observe(document.body, { childList: true, subtree: true });

  // Copy: the answer as plain text plus its sources.
  async function copyText(text) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      const area = document.createElement("textarea");
      area.value = text;
      area.setAttribute("readonly", "");
      area.style.position = "fixed";
      area.style.opacity = "0";
      document.body.append(area);
      area.select();
      const ok = document.execCommand("copy");
      area.remove();
      return ok;
    }
  }
  // Generic: <button data-copy-from="template-id"> copies that template's text.
  document.addEventListener("click", async (event) => {
    const button = event.target.closest?.("[data-copy-from]");
    if (!button) return;
    const text = document.getElementById(button.dataset.copyFrom)?.content?.textContent;
    if (!text) return;
    if (await copyText(text)) window.DS?.toast("success", "Copiado.");
    else window.DS?.toast("danger", "Não foi possível copiar.");
  });
  document.addEventListener("click", async (event) => {
    const button = event.target.closest?.("[data-copy-answer]");
    if (!button) return;
    const text = button.closest("[data-answer]")?.querySelector("template[data-copy-text]")?.content.textContent;
    if (!text) return;
    if (await copyText(text)) window.DS?.toast("success", "Resposta copiada.");
    else window.DS?.toast("danger", "Não foi possível copiar.");
  });

  // ── Navigation: focus the new page title after a section swap (screen readers) ─
  // (HTMX 4 fires swap events on the clicked element, so watch #content itself.)
  const content = document.getElementById("content");
  if (content) {
    new MutationObserver((mutations) => {
      if (!mutations.some((m) => m.target === content)) return;
      const heading = content.querySelector("[data-page-title]");
      heading?.focus({ preventScroll: true });
      if (heading?.textContent) document.title = `${heading.textContent.trim()} · NLMX`;
    }).observe(content, { childList: true });
  }

  // ── Shortcuts: ⌘1…⌘5 switch sections ─────────────────────────────────────────
  document.addEventListener("keydown", (event) => {
    if (!event.metaKey || event.altKey || event.ctrlKey || event.shiftKey) return;
    const link = document.querySelector(`#sidebar-nav [data-shortcut-index="${event.key}"]`);
    if (link) {
      event.preventDefault();
      link.click();
    }
  });
})();
