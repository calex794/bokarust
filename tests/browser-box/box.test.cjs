"use strict";
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { chromium } = require("playwright-core");
const { Client } = require("pg");
const { randomUUID } = require("node:crypto");

test("actual browser: author publishes, guest reads, member saves place", {timeout: 90000}, async () => {
  const base = process.env.BOKARUST_TEST_URL || "http://127.0.0.1:3000";
  const url = new URL(base);
  assert.equal(url.hostname, "127.0.0.1", "browser test must target loopback");
  const dbUrl = process.env.TEST_DATABASE_URL;
  assert.ok(dbUrl, "TEST_DATABASE_URL required");
  const db = new Client({connectionString: dbUrl});
  await db.connect();
  const {rows} = await db.query("SELECT current_database() AS name");
  assert.ok(["bokarust_ci","bokarust_test"].includes(rows[0].name), "refusing non-test database");
  const browser = await chromium.launch({headless:true, executablePath:process.env.CHROMIUM_PATH || "/usr/bin/google-chrome", args:["--no-sandbox"]});
  const context = await browser.newContext({baseURL:base});
  const author = await context.newPage();
  const guest = await browser.newContext({baseURL:base});
  const guestPage = await guest.newPage();
  const suffix = randomUUID().slice(0,12);
  const bookTitle = "Browser box " + suffix;
  const chapterTitle = "Chapter one " + suffix;
  const body = "First paragraph " + suffix + "\n\nSecond paragraph " + suffix;
  try {
    await author.goto("/");
    await assert.equal(await author.getByRole("link",{name:"Каталог"}).first().count(),1);
    await author.goto("/register");
    await author.locator('[name="display_name"]').fill("Browser Author");
    await author.locator('[name="email"]').fill("browser-"+suffix+"@example.invalid");
    await author.locator('[name="password"]').fill("safe-test-password-123");
    const csrfToken = await author.locator('input[name="csrf_token"]').inputValue();
    const storedCookies = await context.cookies(base);
    const csrfCookie = storedCookies.find(cookie => cookie.name === "bkr-dev-csrf");
    assert.ok(csrfCookie, "Chromium did not store the registration CSRF cookie");
    assert.equal(csrfToken, csrfCookie.value, "CSRF cookie and form token disagree");
    const requests = [];
    author.on("request", request => {
      if (request.method() === "POST" && new URL(request.url()).pathname === "/register") {
        const headers = request.headers();
        requests.push({
          origin: headers.origin || "",
          hasCsrfCookie: (headers.cookie || "").includes("bkr-dev-csrf="),
          secFetchSite: headers["sec-fetch-site"] || ""
        });
      }
    });
    const [registration] = await Promise.all([
      author.waitForResponse(response => response.request().method() === "POST" &&
        new URL(response.url()).pathname === "/register"),
      author.getByRole("button", {name:"Создать учётную запись"}).click({noWaitAfter:true})
    ]);
    const registrationStatus = registration.status();
    if (registrationStatus !== 303) {
      const details = await registration.text().catch(() => "<response body unavailable>");
      assert.fail("Registration response: HTTP " + registrationStatus +
        " request=" + JSON.stringify(requests) + " " + details.slice(0,500));
    }
    await author.waitForURL("**/me");

    assert.match(await author.locator("body").innerText(),/Browser Author/);
    await author.goto("/books/new");
    await author.locator('[name="source_identity"]').fill("e2e:"+suffix);
    await author.locator('[name="target_language"]').fill("ru");
    await author.locator('[name="title"]').fill(bookTitle);
    await Promise.all([author.waitForURL(/\/me\/books\/\d+$/),author.getByRole("button",{name:"Создать книгу"}).click()]);
    const bookId = Number(new URL(author.url()).pathname.split("/").pop());
    assert.ok(bookId > 0);
    await author.getByRole("link",{name:"Добавить главу"}).click();
    await author.locator('[name="number"]').fill("1");
    await author.locator('[name="title"]').fill(chapterTitle);
    await author.locator('[name="body_markdown"]').fill(body);
    await author.locator('[name="access_level"]').selectOption("free");
    await author.locator('[name="price_amount"]').fill("0");
    await Promise.all([author.waitForURL(new RegExp("/me/books/"+bookId+"$")),author.getByRole("button",{name:"Сохранить и опубликовать"}).click()]);
    await guestPage.goto("/catalog");
    await guestPage.getByRole("link",{name:bookTitle}).click();
    assert.match(guestPage.url(),new RegExp("/books/"+bookId+"$"));
    await guestPage.getByRole("link",{name:new RegExp(chapterTitle)}).click();
    assert.match(await guestPage.locator("article.chapter-content").innerText(),new RegExp(suffix));
    const chapterId = Number(new URL(guestPage.url()).pathname.split("/").pop());
    assert.ok(chapterId > 0);
    await author.goto("/read/"+chapterId);
    await author.getByRole("button",{name:"Запомнить место здесь"}).last().click();
    await author.waitForURL("**/read/"+chapterId+"#p-2");
    await author.goto("/me/library");
    await author.getByRole("link",{name:"Продолжить чтение"}).click();
    assert.match(author.url(),new RegExp("/read/"+chapterId+"#p-2$"));
    assert.match(await author.locator("body").innerText(),/К сохранённому месту/);
    const hidden = await db.query("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level) VALUES ($1,2,$2,'draft','free') RETURNING id",[bookId,"Unpublished "+suffix]);
    await guestPage.goto("/books/"+bookId);
    assert.ok(!(await guestPage.locator("body").innerText()).includes("Unpublished "+suffix));
    const draftResponse=await guestPage.goto("/reader/"+hidden.rows[0].id);
    assert.equal(draftResponse.status(),404);
  } finally {
    try {
      await guest.close();
      await context.close();
      await browser.close();
    } finally {
      // The isolated CI database is disposed of after the job; never production.
      await db.end();
    }
  }
});
