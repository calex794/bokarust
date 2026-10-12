/* BKR-88: small progressive enhancement. Manual place buttons work without JS. */
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
      action.origin !== window.location.origin || !action.pathname.endsWith("/place")) {
    return;
  }
  const endpoint = action.pathname + "/auto";
  const quietMs = 1400;
  const minIntervalMs = 7000;
  const saved = document.querySelector('.reader-return a[href^="#p-"]');
  const savedNumber = saved ? Number(saved.getAttribute("href").slice(3)) : 0;
  let confirmed = Number.isInteger(savedNumber) && savedNumber > 0 ? savedNumber : 0;
  let pending = 0;
  let lastAttempt = -minIntervalMs;
  let timer = null;
  let framePending = false;
  let sending = false;
  let engaged = false;
  let stopped = false;

  status.textContent = "Место сохраняется автоматически при прокрутке.";

  // Find the block crossing the upper reading area. Binary search avoids a
  // full DOM scan even at the maximum 200 blocks per chapter.
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

  function payload(index) {
    return new URLSearchParams({
      csrf_token: csrf,
      version_id: version,
      block_index: String(index),
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
    if (stopped || sending || pending <= confirmed) return;
    const index = pending;
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
        body: payload(index),
      });
      if (response.status === 204) {
        confirmed = Math.max(confirmed, index);
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
      // Never spin on network failures. A subsequent scroll can retry.
      if (succeeded && pending > confirmed && !stopped && !document.hidden) schedule();
    }
  }

  function considerScroll() {
    framePending = false;
    if (!engaged || stopped || document.hidden) return;
    const index = visibleBlock();
    if (index <= confirmed) return;
    pending = Math.max(pending, index);
    schedule();
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

  // Best effort on tab close. The database still rejects stale/older places.
  function flushOnHide() {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    if (!stopped && pending > confirmed && navigator.sendBeacon) {
      const body = new Blob([payload(pending)], {
        type: "application/x-www-form-urlencoded;charset=UTF-8",
      });
      navigator.sendBeacon(endpoint, body);
    }
  }
  window.addEventListener("pagehide", flushOnHide);
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) flushOnHide();
  });
})();
