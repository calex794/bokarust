// Browser behavior without a DOM package or a separate frontend test runner.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const path = require("node:path");

const script = fs.readFileSync(
  path.join(__dirname, "../assets/reader-progress.js"), "utf8"
);

function browser({ responseStatus = 204, saved = 0, savedOffset = 0,
  caretOffset = 0, hash = '', formatted = false, sourceStart = 0 } = {}) {
  const events = {};
  const documentEvents = {};
  const sent = [];
  const beacons = [];
  let top = [-500, 100, 700];
  let currentCaret = caretOffset;
  const scrolls = [];
  const rangeStartOffsets = [];
  const textNode = { nodeType: 3, textContent: "Т😀x".repeat(40) };
  const mark = {
    firstChild: textNode,
    getAttribute(name) { return name === "data-md-start" ? String(sourceStart) : null; },
  };
  if (formatted) textNode.parentElement = {
    closest(selector) { return selector === "[data-md-start]" ? mark : null; },
  };
  const pre = (index) => ({
    firstChild: formatted ? { nodeType: 1 } : textNode,
    querySelectorAll(selector) {
      assert.equal(selector, "[data-md-start]");
      return formatted ? [mark] : [];
    },
    contains(node) { return node === textNode; },
    getBoundingClientRect() {
      return { top: top[index], left: 20, width: 400 };
    },
  });
  let now = 10_000;
  let nextTimer = 0;
  const timers = new Map();
  const status = { textContent: "Use manual buttons" };
  const form = {
    action: "https://reader.example/read/17/place",
    querySelector(selector) {
      if (selector.includes("csrf_token")) return { value: "testcsrf" };
      if (selector.includes("version_id")) return { value: "42" };
      return null;
    },
  };
  const pieces = [0, 1, 2].map((index) => ({
    getBoundingClientRect() { return { top: top[index] }; },
    querySelector() { return pre(index); },
  }));
  const document = {
    hidden: false,
    getElementById(id) {
      return id === "save-position" ? form : id === "reader-save-status" ? status : null;
    },
    querySelectorAll(selector) {
      assert.equal(selector, ".reader-piece");
      return pieces;
    },
    querySelector() {
      return saved ? { getAttribute(name) {
        return name === "data-char-offset" ? String(savedOffset) : "#p-" + saved;
      } } : null;
    },
    caretPositionFromPoint() {
      return { offsetNode: textNode, offset: currentCaret };
    },
    createRange() {
      return {
        setStart(node, offset) {
          this.node = node;
          this.offset = offset;
          rangeStartOffsets.push(offset);
        },
        collapse() {},
        getBoundingClientRect() { return { top: 230 }; },
      };
    },
    addEventListener(name, handler) { documentEvents[name] = handler; },
  };
  const window = {
    location: { href: "https://reader.example/read/17" + hash, origin: "https://reader.example", hash },
    innerHeight: 800,
    scrollBy(x, y) { scrolls.push([x, y]); },
    addEventListener(name, handler) { events[name] = handler; },
    requestAnimationFrame(callback) { callback(); },
  };
  const fetch = async (url, options) => {
    sent.push({ url, options });
    return { status: responseStatus };
  };
  class Clock extends Date {
    static now() { return now; }
  }
  const navigator = {
    sendBeacon(url, body) { beacons.push({ url, body }); return true; },
  };
  const context = {
    document, window, navigator, fetch, Date: Clock, URL, URLSearchParams, Blob,
    setTimeout(callback, delay) {
      const id = ++nextTimer;
      timers.set(id, { callback, delay });
      return id;
    },
    clearTimeout(id) { timers.delete(id); },
  };
  vm.runInNewContext(script, context, { timeout: 1500 });
  return {
    events, documentEvents, status, sent, beacons, document, timers, scrolls,
    rangeStartOffsets,
    setTop(value) { top = value; },
    setCaretOffset(value) { currentCaret = value; },
    setNow(value) { now = value; },
    async fireTimer() {
      const first = timers.keys().next().value;
      assert.notEqual(first, undefined, "should have scheduled the save");
      const entry = timers.get(first);
      timers.delete(first);
      entry.callback();
      await Promise.resolve();
      await Promise.resolve();
      return entry.delay;
    },
  };
}

test("navigation without reader interaction does not save anything", () => {
  const page = browser();
  page.events.scroll();
  assert.equal(page.sent.length, 0);
  assert.equal(page.timers.size, 0);
  assert.match(page.status.textContent, /автоматически/);
});

test("scroll saves a bounded block without redirect or repeated polling", async () => {
  const page = browser();
  page.events.wheel();
  page.events.scroll();
  const delay = await page.fireTimer();
  assert.equal(delay, 1400);
  assert.equal(page.sent.length, 1);
  assert.equal(page.sent[0].url, "/read/17/place/auto");
  assert.equal(page.sent[0].options.credentials, "same-origin");
  assert.equal(page.sent[0].options.redirect, "error");
  const body = new URLSearchParams(page.sent[0].options.body);
  assert.equal(body.get("csrf_token"), "testcsrf");
  assert.equal(body.get("version_id"), "42");
  assert.equal(body.get("block_index"), "2");
  assert.equal(page.status.textContent, "Место сохранено.");
  page.events.scroll();
  assert.equal(page.timers.size, 0, "same place must not trigger a second save");
  page.setTop([-900, -350, 200]);
  page.setNow(10_300);
  page.events.scroll();
  const next = await page.fireTimer();
  assert.ok(next >= 6700, "save frequency must remain limited");
  assert.equal(page.sent.length, 2);
});

test("closing a tab sends only a new position, without extra requests", async () => {
  const page = browser({ saved: 2 });
  page.events.touchstart();
  page.events.scroll();
  assert.equal(page.timers.size, 0, "saved place should not be resent");
  page.setTop([-900, -350, 120]);
  page.events.scroll();
  assert.equal(page.timers.size, 1);
  page.events.pagehide();
  assert.equal(page.timers.size, 0);
  assert.equal(page.beacons.length, 1);
  assert.equal(page.beacons[0].url, "/read/17/place/auto");
  const payload = new URLSearchParams(await page.beacons[0].body.text());
  assert.equal(payload.get("block_index"), "3");
  assert.equal(page.sent.length, 0);
});

test("stale published version stops autosaving and reports a recoverable error", async () => {
  const page = browser({ responseStatus: 409 });
  page.events.keydown({ key: "PageDown" });
  page.events.scroll();
  await page.fireTimer();
  assert.match(page.status.textContent, /Перезагрузите страницу/);
  page.setTop([-900, -350, 100]);
  page.events.scroll();
  assert.equal(page.timers.size, 0);
  assert.equal(page.sent.length, 1);
});

test("network failure requires another scroll instead of retrying forever", async () => {
  const page = browser({ responseStatus: 503 });
  page.events.wheel();
  page.events.scroll();
  await page.fireTimer();
  assert.equal(page.timers.size, 0);
  page.events.scroll();
  assert.equal(page.timers.size, 1);
  assert.match(page.status.textContent, /следующей прокрутке/);
});


test("within one block the UTF-16 offset advances and is stored", async () => {
  const page = browser({ caretOffset: 3 });
  page.events.wheel();
  page.events.scroll();
  await page.fireTimer();
  let payload = new URLSearchParams(page.sent[0].options.body);
  assert.equal(payload.get("block_index"), "2");
  assert.equal(payload.get("char_offset"), "3"); // UTF-16 position after an emoji
  page.setCaretOffset(9);
  page.events.scroll();
  await page.fireTimer();
  payload = new URLSearchParams(page.sent[1].options.body);
  assert.equal(payload.get("block_index"), "2");
  assert.equal(payload.get("char_offset"), "9");
  page.setCaretOffset(2);
  page.events.scroll();
  assert.equal(page.timers.size, 0, "older place in the same paragraph is not resent");
});

test("fragment restores a saved character offset without writing it again", () => {
  const page = browser({ saved: 2, savedOffset: 12, hash: "#p-2" });
  assert.equal(page.scrolls.length, 0);
  page.events.load();
  assert.equal(page.scrolls.length, 1);
  assert.equal(page.scrolls[0][0], 0);
  page.events.scroll();
  assert.equal(page.timers.size, 0, "programmatic restore must never save");
  const unrelated = browser({ saved: 2, savedOffset: 12, hash: "#p-1" });
  unrelated.events.load();
  assert.equal(unrelated.scrolls.length, 0);
});


test("formatted Markdown saves source offset rather than HTML text-node offset", async () => {
  const page = browser({ formatted: true, sourceStart: 9, caretOffset: 4 });
  page.events.wheel();
  page.events.scroll();
  await page.fireTimer();
  const body = new URLSearchParams(page.sent[0].options.body);
  assert.equal(body.get("block_index"), "2");
  assert.equal(body.get("char_offset"), "13");
});

test("existing precise bookmark restores inside formatted text after markup", () => {
  const page = browser({
    formatted: true, sourceStart: 9, saved: 2, savedOffset: 15, hash: "#p-2",
  });
  page.events.load();
  assert.equal(page.scrolls.length, 1);
  assert.deepEqual(page.rangeStartOffsets, [6]);
  page.events.scroll();
  assert.equal(page.sent.length, 0);
});

test("caret outside reader markup cannot produce a false source offset", async () => {
  const page = browser({ formatted: true, sourceStart: 30, caretOffset: 2 });
  page.events.wheel();
  page.document.caretPositionFromPoint = () => ({
    offsetNode: { nodeType: 3, textContent: "Outside navigation" }, offset: 7,
  });
  page.events.scroll();
  await page.fireTimer();
  const body = new URLSearchParams(page.sent[0].options.body);
  assert.equal(body.get("char_offset"), "0");
});
