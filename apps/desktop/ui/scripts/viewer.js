// PDF viewer beside the chat: pages rendered by PDFium on demand, a transparent text layer for
// selection, search, highlights, thumbnails, zoom/fit width, keyboard shortcuts, a resizable
// pane and full screen. Opened by HTMX into #viewer (citations, sources, page references).
(() => {
  "use strict";

  const ZOOMS = [0.5, 0.67, 0.75, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3];
  const WIDTHS = [400, 600, 900, 1200, 1600, 2000, 2400];
  const PX_PER_PT = 96 / 72;
  // Interface strings come from the page's i18n block (`window.nlmxT`, defined in app.js).
  const tr = (key, fallback) => (typeof window.nlmxT === "function" ? window.nlmxT(key) : "") || fallback;
  const store = {
    get(key) {
      try { return JSON.parse(localStorage.getItem(key)); } catch { return null; }
    },
    set(key, value) {
      try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* private mode */ }
    },
  };
  const chatPage = () => document.querySelector("[data-chat-page]");
  const openViewer = () => document.querySelector("#viewer [data-viewer]");
  let lastTrigger = null;

  function init(root) {
    if (root.dataset.ready !== undefined) return;
    root.dataset.ready = "";
    const doc = root.dataset.document;
    const scroll = root.querySelector("[data-viewer-scroll]");
    const pagesBox = root.querySelector("[data-viewer-pages]");
    const pages = [...root.querySelectorAll(".viewer-page")];
    const thumbs = [...root.querySelectorAll("[data-goto-page]")];
    const pageInput = root.querySelector("[data-viewer-page]");
    const zoomLabel = root.querySelector('[data-viewer-zoom="0"]');
    const fitButton = root.querySelector("[data-viewer-fit]");
    const thumbsButton = root.querySelector("[data-viewer-thumbs]");
    const searchInput = root.querySelector("[data-viewer-search-input]");
    const searchCount = root.querySelector("[data-viewer-search-count]");
    const saved = store.get(`viewer-zoom-${doc}`) || { fit: true, zoom: 1 };
    const state = {
      fit: saved.fit,
      zoom: saved.zoom,
      effective: 1,
      current: Number(root.dataset.targetPage) || 1,
      query: "",
      hits: [],
      hit: -1,
    };
    const visible = new Set();

    if (store.get("viewer-thumbs-hidden")) {
      root.dataset.thumbsHidden = "";
      thumbsButton?.setAttribute("aria-pressed", "false");
    }

    // ── Zoom ──
    const widest = () => Math.max(...pages.map((p) => Number(p.dataset.width) || 612));
    const fitZoom = () => Math.max(0.25, Math.min(4, (scroll.clientWidth - 34) / (widest() * PX_PER_PT)));
    function applyZoom(keepPosition = true) {
      const page = pages[state.current - 1];
      const offset = page && page.offsetHeight ? (scroll.scrollTop - page.offsetTop) / page.offsetHeight : 0;
      state.effective = state.fit ? fitZoom() : state.zoom;
      pagesBox.style.setProperty("--scale", String(state.effective * PX_PER_PT));
      if (zoomLabel) zoomLabel.textContent = `${Math.round(state.effective * 100)}%`;
      fitButton?.setAttribute("aria-pressed", String(state.fit));
      if (keepPosition && page) scroll.scrollTop = page.offsetTop + offset * page.offsetHeight;
      visible.forEach((p) => { loadImage(p); fitText(p); });
      store.set(`viewer-zoom-${doc}`, { fit: state.fit, zoom: state.zoom });
    }
    function stepZoom(direction) {
      const current = state.effective;
      const next = direction > 0 ? ZOOMS.find((z) => z > current + 0.001) : [...ZOOMS].reverse().find((z) => z < current - 0.001);
      state.fit = false;
      state.zoom = next ?? current;
      applyZoom();
    }

    // ── Pages: image at a width matching the zoom, text layer, on demand ──
    // Decided by the image's actual src (never by a marker attribute: HTMX copies attributes
    // between elements with the same id while settling).
    function loadImage(page, force = false) {
      const img = page.querySelector(".viewer-page-image");
      if (!img || page.clientWidth === 0) return;
      const want = WIDTHS.find((w) => w >= page.clientWidth * (window.devicePixelRatio || 1)) || WIDTHS.at(-1);
      const url = `${img.dataset.srcBase}&w=${want}`;
      if (force || img.getAttribute("src") !== url) {
        page.querySelector(".viewer-page-failed")?.setAttribute("hidden", "");
        page.classList.add("is-loading");
        img.onload = () => page.classList.remove("is-loading");
        img.onerror = () => {
          page.classList.remove("is-loading");
          page.querySelector(".viewer-page-failed")?.removeAttribute("hidden");
        };
        img.src = force ? `${url}&retry=${Date.now()}` : url;
      }
    }
    function fitText(page) {
      const layer = page.querySelector(".viewer-text");
      const spans = layer ? [...layer.children] : [];
      if (!spans.length) return;
      const height = page.clientHeight;
      const width = page.clientWidth;
      for (const s of spans) {
        s.style.transform = "";
        s.style.fontSize = `${(Number(s.dataset.h) / 100) * height * 0.92}px`;
      }
      const natural = spans.map((s) => s.getBoundingClientRect().width);
      spans.forEach((s, i) => {
        const target = (Number(s.dataset.w) / 100) * width;
        if (natural[i] > 0) s.style.transform = `scaleX(${target / natural[i]})`;
      });
    }
    async function loadText(page) {
      const layer = page.querySelector(".viewer-text");
      if (!layer || layer.dataset.loaded !== undefined) return;
      layer.dataset.loaded = "";
      try {
        const response = await fetch(layer.dataset.textSrc);
        if (response.ok) {
          layer.innerHTML = await response.text();
          fitText(page);
        }
      } catch { /* the page stays viewable without selection */ }
    }
    const pageObserver = new IntersectionObserver((entries) => {
      for (const e of entries) {
        if (e.isIntersecting) {
          visible.add(e.target);
          loadImage(e.target);
          loadText(e.target);
        } else {
          visible.delete(e.target);
        }
      }
    }, { root: scroll, rootMargin: "100% 0px" });
    pages.forEach((p) => pageObserver.observe(p));

    const thumbObserver = new IntersectionObserver((entries) => {
      for (const e of entries) {
        const img = e.target.querySelector("img[data-thumb-src]");
        if (e.isIntersecting && img && !img.src) img.src = img.dataset.thumbSrc;
      }
    }, { root: root.querySelector("[data-viewer-thumb-list]"), rootMargin: "200px 0px" });
    thumbs.forEach((t) => thumbObserver.observe(t));

    // ── Navigation and current page ──
    function setCurrent(n) {
      if (n === state.current && pageInput?.value === String(n)) return;
      state.current = n;
      if (pageInput && document.activeElement !== pageInput) pageInput.value = String(n);
      thumbs.forEach((t) => {
        const on = Number(t.dataset.gotoPage) === n;
        if (on) {
          t.setAttribute("aria-current", "page");
          t.scrollIntoView({ block: "nearest" });
        } else {
          t.removeAttribute("aria-current");
        }
      });
    }
    function goTo(n, { anchor = null, smooth = true } = {}) {
      const target = Math.min(Math.max(1, n), pages.length);
      const page = pages[target - 1];
      if (!page) return;
      const top = anchor == null ? page.offsetTop - 16 : page.offsetTop + (anchor / 100) * page.offsetHeight - 56;
      scroll.scrollTo({ top: Math.max(0, top), behavior: smooth ? "smooth" : "auto" });
      setCurrent(target);
    }
    let ticking = false;
    scroll.addEventListener("scroll", () => {
      if (ticking) return;
      ticking = true;
      requestAnimationFrame(() => {
        ticking = false;
        const line = scroll.scrollTop + scroll.clientHeight / 3;
        let n = 1;
        for (const p of pages) {
          if (p.offsetTop <= line) n = Number(p.dataset.page);
          else break;
        }
        setCurrent(n);
      });
    });

    // ── Search ──
    function clearHits() {
      root.querySelectorAll(".viewer-hit").forEach((h) => h.remove());
    }
    function showHit(i) {
      if (!state.hits.length) return;
      state.hit = (i + state.hits.length) % state.hits.length;
      root.querySelectorAll(".viewer-hit.is-current").forEach((h) => h.classList.remove("is-current"));
      root.querySelectorAll(`.viewer-hit[data-hit="${state.hit}"]`).forEach((h) => h.classList.add("is-current"));
      const hit = state.hits[state.hit];
      goTo(hit.page, { anchor: hit.boxes[0]?.top ?? null });
      if (searchCount) searchCount.textContent = `${state.hit + 1} ${tr("js-viewer-of", "of")} ${state.hits.length}${state.truncated ? "+" : ""}`;
    }
    async function search(query, direction) {
      if (!query) {
        state.query = "";
        state.hits = [];
        clearHits();
        if (searchCount) searchCount.textContent = "";
        return;
      }
      if (query === state.query && state.hits.length) {
        showHit(state.hit + direction);
        return;
      }
      state.query = query;
      if (searchCount) searchCount.textContent = tr("js-viewer-searching", "Searching…");
      try {
        const response = await fetch(`/viewer/${doc}/search?q=${encodeURIComponent(query)}`);
        const results = response.ok ? await response.json() : { hits: [] };
        state.hits = results.hits || [];
        state.truncated = results.truncated;
      } catch {
        state.hits = [];
      }
      clearHits();
      state.hits.forEach((hit, i) => {
        const marks = pages[hit.page - 1]?.querySelector("[data-viewer-marks]");
        for (const b of hit.boxes) {
          const mark = document.createElement("span");
          mark.className = "viewer-hit";
          mark.dataset.hit = String(i);
          Object.assign(mark.style, { left: `${b.left}%`, top: `${b.top}%`, width: `${b.width}%`, height: `${b.height}%` });
          marks?.append(mark);
        }
      });
      if (state.hits.length) {
        // Start from the first result at or after the current page.
        const start = state.hits.findIndex((h) => h.page >= state.current);
        showHit(start < 0 ? 0 : start);
      } else if (searchCount) {
        searchCount.textContent = tr("js-viewer-none", "None");
      }
    }

    // ── Controls ──
    root.addEventListener("click", (event) => {
      const t = event.target.closest("button");
      if (!t || !root.contains(t)) return;
      if (t.matches("[data-viewer-retry]")) loadImage(t.closest(".viewer-page"), true);
      else if (t.matches("[data-viewer-prev]")) goTo(state.current - 1);
      else if (t.matches("[data-viewer-next]")) goTo(state.current + 1);
      else if (t.matches("[data-goto-page]")) goTo(Number(t.dataset.gotoPage));
      else if (t.matches("[data-viewer-zoom]")) {
        const step = Number(t.dataset.viewerZoom);
        if (step === 0) {
          state.fit = false;
          state.zoom = 1;
          applyZoom();
        } else {
          stepZoom(step);
        }
      } else if (t.matches("[data-viewer-fit]")) {
        if (state.fit) state.zoom = state.effective;
        state.fit = !state.fit;
        applyZoom();
      } else if (t.matches("[data-viewer-thumbs]")) {
        const hidden = root.toggleAttribute("data-thumbs-hidden");
        t.setAttribute("aria-pressed", String(!hidden));
        store.set("viewer-thumbs-hidden", hidden);
        if (state.fit) requestAnimationFrame(() => applyZoom());
      } else if (t.matches("[data-viewer-expand]")) {
        const expanded = chatPage()?.toggleAttribute("data-viewer-expanded");
        t.setAttribute("aria-pressed", String(Boolean(expanded)));
        requestAnimationFrame(() => applyZoom());
      } else if (t.matches("[data-viewer-hit]")) {
        search(searchInput?.value.trim() || "", Number(t.dataset.viewerHit));
      }
    });
    pageInput?.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        const n = parseInt(pageInput.value, 10);
        if (Number.isFinite(n)) goTo(n);
        else pageInput.value = String(state.current);
      }
    });
    pageInput?.addEventListener("blur", () => { pageInput.value = String(state.current); });
    root.querySelector("[data-viewer-search]")?.addEventListener("submit", (event) => event.preventDefault());
    searchInput?.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        search(searchInput.value.trim(), event.shiftKey ? -1 : 1);
      } else if (event.key === "Escape" && searchInput.value) {
        event.stopPropagation();
        searchInput.value = "";
        search("", 1);
      }
    });

    root.viewerApi = {
      zoom: stepZoom,
      fit() { state.fit = true; applyZoom(); },
      prev() { goTo(state.current - 1); },
      next() { goTo(state.current + 1); },
      focusSearch() { searchInput?.focus(); searchInput?.select(); },
      relayout() { if (state.fit) applyZoom(); },
    };

    new ResizeObserver(() => { if (state.fit) applyZoom(); }).observe(scroll);

    // ── Open at the target: page, position on the passage, highlight ──
    requestAnimationFrame(() => {
      applyZoom(false);
      const anchor = root.dataset.anchor === "" ? null : Number(root.dataset.anchor);
      goTo(state.current, { anchor, smooth: false });
      if (!("hasHighlights" in root.dataset) && root.querySelector(".viewer-origin")) {
        pages[state.current - 1]?.classList.add("is-flash");
      }
    });
  }

  function initAll() {
    document.querySelectorAll("[data-viewer]").forEach(init);
    restoreWidth();
  }

  // ── Pane: open, close, resize, full screen ──
  function closeViewer() {
    const page = chatPage();
    page?.removeAttribute("data-panel-open");
    page?.removeAttribute("data-viewer-expanded");
    const pane = document.getElementById("viewer");
    if (pane) pane.innerHTML = "";
    if (lastTrigger?.isConnected) lastTrigger.focus();
    lastTrigger = null;
  }
  document.addEventListener("click", (event) => {
    const trigger = event.target.closest?.('[hx-target="#viewer"]');
    if (trigger && !trigger.closest("#viewer")) lastTrigger = trigger;
    if (event.target.closest?.("[data-close-panel]")) closeViewer();
  });
  // HTMX 4 fires swap events on the element that was clicked, not on the target, so the pane
  // follows its content instead: something swapped into #viewer opens it, emptying closes it.
  function syncPane(pane) {
    const page = pane.closest("[data-chat-page]");
    if (!page) return;
    const content = pane.querySelector("[data-viewer], [data-source-info], .viewer-error");
    if (!content) {
      page.removeAttribute("data-panel-open");
      page.removeAttribute("data-viewer-expanded");
      return;
    }
    page.setAttribute("data-panel-open", "");
    // After HTMX's settle step has restored the new elements' own attributes.
    requestAnimationFrame(() => {
      initAll();
      if (!pane.contains(document.activeElement)) {
        content.querySelector("[data-panel-title]")?.focus({ preventScroll: true });
      }
    });
  }
  new MutationObserver((mutations) => {
    const panes = new Set();
    for (const m of mutations) {
      if (m.target instanceof HTMLElement && m.target.id === "viewer") panes.add(m.target);
    }
    panes.forEach(syncPane);
  }).observe(document.body, { childList: true, subtree: true });
  document.addEventListener("htmx:after:settle", initAll);
  document.addEventListener("DOMContentLoaded", initAll);
  if (document.readyState !== "loading") initAll();

  function restoreWidth() {
    const width = store.get("viewer-width");
    const main = document.querySelector(".chat-main");
    if (main && width) main.style.setProperty("--viewer-width", `${width}px`);
  }
  function setWidth(px) {
    const main = document.querySelector(".chat-main");
    if (!main) return;
    const max = main.clientWidth - 256;
    const width = Math.round(Math.min(Math.max(352, px), max));
    main.style.setProperty("--viewer-width", `${width}px`);
    store.set("viewer-width", width);
    openViewer()?.viewerApi?.relayout();
  }
  document.addEventListener("pointerdown", (event) => {
    const handle = event.target.closest?.("[data-viewer-resizer]");
    if (!handle) return;
    event.preventDefault();
    handle.setPointerCapture(event.pointerId);
    const main = handle.closest(".chat-main");
    const move = (e) => setWidth(main.getBoundingClientRect().right - e.clientX);
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
  });

  // ── Keyboard ──
  document.addEventListener("keydown", (event) => {
    const viewer = openViewer();
    if (!viewer?.viewerApi) return;
    const api = viewer.viewerApi;
    const resizer = event.target.closest?.("[data-viewer-resizer]");
    if (resizer && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
      event.preventDefault();
      const pane = document.getElementById("viewer");
      setWidth((pane?.clientWidth || 480) + (event.key === "ArrowLeft" ? 24 : -24));
      return;
    }
    if (event.metaKey && !event.altKey && !event.ctrlKey) {
      if (event.key === "f") { event.preventDefault(); api.focusSearch(); return; }
      // Zoom shortcuts act on the document while it has focus.
      if (!viewer.contains(document.activeElement)) return;
      if (event.key === "=" || event.key === "+") { event.preventDefault(); api.zoom(1); }
      else if (event.key === "-") { event.preventDefault(); api.zoom(-1); }
      else if (event.key === "0") { event.preventDefault(); api.fit(); }
      return;
    }
    const typing = event.target.closest?.("input, textarea, select, [contenteditable]");
    if (event.key === "Escape" && !document.querySelector("dialog[open]") && !typing) {
      const page = chatPage();
      if (page?.hasAttribute("data-viewer-expanded")) {
        page.removeAttribute("data-viewer-expanded");
        viewer.querySelector("[data-viewer-expand]")?.setAttribute("aria-pressed", "false");
        requestAnimationFrame(() => api.relayout());
      } else {
        closeViewer();
      }
      return;
    }
    if (typing || !viewer.contains(document.activeElement)) return;
    if (event.key === "ArrowLeft" || event.key === "PageUp") { event.preventDefault(); api.prev(); }
    else if (event.key === "ArrowRight" || event.key === "PageDown") { event.preventDefault(); api.next(); }
  });
})();
