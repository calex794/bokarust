/* BKR-89: deterministic UTF-16 place within immutable chapter text. */
(() => {
  "use strict";

  const form = document.getElementById("save-position");
  const status = document.getElementById("reader-save-status");
  const pieces = Array.from(document.querySelectorAll(".reader-piece"));
  if (!form || !status || pieces.length === 0) return;

  const csrf = form.querySelector('input[name="csrf_token"]')?.value;
  const version = form.querySelector('input[name="version_id"]')?.value;
  const action = new URL(form.action, window.location.href);
  if (!csrf || !/^[0-9]+$/.test(version || "") ||
      action.origin !== window.location.origin || !action.pathname.endsWith("/place")) return;
  const endpoint = action.pathname + "/auto";

  const quietMs = 1400;
  const minIntervalMs = 7000;
  const saved = document.querySelector('.reader-return a[href^="#p-"]');
  const savedNumber = saved ? Number(saved.getAttribute("href").slice(3)) : 0;
  const savedOffset = saved ? Number(saved.getAttribute("data-char-offset") || 0) : 0;
  const initialBlock = Number.isInteger(savedNumber) && savedNumber > 0 ? savedNumber : 0;
  const initialOffset = Number.isInteger(savedOffset) && savedOffset >= 0 ? savedOffset : 0;
  let confirmed = { block: initialBlock, offset: initialOffset };
  let pending = { block: 0, offset: 0 };
  let lastAttempt = -minIntervalMs;
  let timer = null;
  let framePending = false;
  let sending = false;
  let engaged = false;
  let stopped = false;

  status.textContent = "Место сохраняется автоматически при прокрутке.";

  function ahead(a, b) {
    return a.block > b.block || (a.block === b.block && a.offset > b.offset);
  }

  function textNode(index) {
    const element = pieces[index - 1]?.querySelector(".reader-piece__text");
    const node = element?.firstChild;
    if (!node || node.nodeType !== 3) return null;
    return { element, node };
  }

  // Binary search costs O(log N), where N never exceeds 200.
  function visibleBlock() {
    const threshold = Math.min(300, window.innerHeight * 0.42);
    let left = 0;
    let right = pieces.length - 1;
    let found = 0;
    while (left <= right) {
      const mid = Math.floor((left + right) / 2);
      if (pieces[mid].getBoundingClientRect().top <= threshold) {
        found = mid;
        left = mid + 1;
      } else {
        right = mid - 1;
      }
    }
    return found + 1;
  }

  function visiblePlace() {
    const block = visibleBlock();
    const part = textNode(block);
    if (!part) return { block, offset: 0 };
    const rect = part.element.getBoundingClientRect();
    const x = rect.left + Math.min(Math.max(8, rect.width / 2), Math.max(8, rect.width - 1));
    const y = Math.max(rect.top + 1, Math.min(window.innerHeight - 2, window.innerHeight * 0.42));
    let node = null;
    let offset = 0;
    if (typeof document.caretPositionFromPoint === "function") {
      const position = document.caretPositionFromPoint(x, y);
      node = position?.offsetNode;
      offset = position?.offset;
    } else if (typeof document.caretRangeFromPoint === "function") {
      const range = document.caretRangeFromPoint(x, y);
      node = range?.startContainer;
      offset = range?.startOffset;
    }
    // Never treat positions in navigation, buttons or other blocks as text.
    if (node !== part.node || !Number.isInteger(offset)) return { block, offset: 0 };
    return { block, offset: Math.max(0, Math.min(offset, part.node.textContent.length)) };
  }

  // Only restore a bookmarked fragment, not an unrelated deep link or new visit.
  function restorePlace() {
    if (!saved || initialBlock < 1 || initialOffset < 1 ||
        window.location.hash !== "#p-" + initialBlock ||
        typeof document.createRange !== "function") return;
    const part = textNode(initialBlock);
    if (!part) return;
    const range = document.createRange();
    range.setStart(part.node, Math.min(initialOffset, part.node.textContent.length));
    range.collapse(true);
    const rect = range.getBoundingClientRect();
    if (rect && Number.isFinite(rect.top)) {
      window.scrollBy(0, rect.top - Math.min(160, window.innerHeight * 0.28));
    }
  }

  function payload(place) {
    return new URLSearchParams({
      csrf_token: csrf,
      version_id: version,
      block_index: String(place.block),
      char_offset: String(place.offset),
    }).toString();
  }

  function schedule() {
    if (timer !== null) clearTimeout(timer);
    const remaining = Math.max(0, minIntervalMs - (Date.now() - lastAttempt));
    timer = setTimeout(() => {
      timer = null;
      if (!document.hidden) void save();
    }, Math.max(quietMs, remaining));
  }

  async function save() {
    if (stopped || sending || !ahead(pending, confirmed)) return;
    const place = pending;
    lastAttempt = Date.now();
    sending = true;
    status.textContent = "Сохраняю место…";
    let succeeded = false;
    try {
      const response = await fetch(endpoint, {
        method: "POST",
        credentials: "same-origin",
        redirect: "error",
        cache: "no-store",
        headers: { "Content-Type": "application/x-www-form-urlencoded;charset=UTF-8" },
        body: payload(place),
      });
      if (response.status === 204) {
        if (ahead(place, confirmed)) confirmed = place;
        status.textContent = "Место сохранено.";
        succeeded = true;
      } else if (response.status === 409) {
        stopped = true;
        status.textContent = "Глава обновилась. Перезагрузите страницу, чтобы продолжить.";
      } else if ([401, 403, 404].includes(response.status)) {
        stopped = true;
        status.textContent = "Сохранение недоступно. Обновите страницу или войдите заново.";
      } else {
        status.textContent = "Не удалось сохранить место. Повторим при следующей прокрутке.";
      }
    } catch {
      status.textContent = "Нет связи. Повторим при следующей прокрутке.";
    } finally {
      sending = false;
      if (succeeded && ahead(pending, confirmed) && !stopped && !document.hidden) schedule();
    }
  }

  function considerScroll() {
    framePending = false;
    if (!engaged || stopped || document.hidden) return;
    const place = visiblePlace();
    if (!ahead(place, confirmed)) return;
    if (ahead(place, pending)) pending = place;
    if (!sending) schedule();
  }

  function engage() { engaged = true; }
  window.addEventListener("wheel", engage, { passive: true });
  window.addEventListener("touchstart", engage, { passive: true });
  window.addEventListener("pointerdown", engage, { passive: true });
  window.addEventListener("keydown", (event) => {
    if (["ArrowDown", "ArrowUp", "PageDown", "PageUp", "Home", "End", " "]
        .includes(event.key)) engage();
  });
  window.addEventListener("scroll", () => {
    if (!engaged || stopped || framePending) return;
    framePending = true;
    window.requestAnimationFrame(considerScroll);
  }, { passive: true });

  function flushOnHide() {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    if (!stopped && ahead(pending, confirmed) && navigator.sendBeacon) {
      const body = new Blob([payload(pending)], {
        type: "application/x-www-form-urlencoded;charset=UTF-8",
      });
      navigator.sendBeacon(endpoint, body);
    }
  }
  window.addEventListener("load", restorePlace);
  window.addEventListener("hashchange", restorePlace);
  window.addEventListener("pagehide", flushOnHide);
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) flushOnHide();
  });
})();
