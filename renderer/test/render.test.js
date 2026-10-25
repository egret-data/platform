// Exercises the real service against a throwaway fixture HTTP server and a
// real (headless) Chromium via Playwright — no mocking of Playwright, no
// dependency on egret itself being up. Fixture pages live entirely in this
// file, served from an ephemeral `node:http` port.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createApp, closeBrowser } from "../server.js";

const PNG_ICON_LIMIT = 64; // the repo's PNG-export lesson: 16x16/32x32 = "grabbed an icon"

const FIXTURES = {
  // Draws real (high-entropy, so it can't collapse to a trivial-size PNG)
  // content, then signals readiness after a short async delay — mirrors a
  // real chart's draw-then-flag lifecycle.
  "/ready": `<!doctype html><html><body style="margin:0">
    <canvas id="c" width="640" height="480"></canvas>
    <script>
      const ctx = document.getElementById("c").getContext("2d");
      const img = ctx.createImageData(640, 480);
      for (let i = 0; i < img.data.length; i += 4) {
        img.data[i] = Math.floor(Math.random() * 256);
        img.data[i + 1] = Math.floor(Math.random() * 256);
        img.data[i + 2] = Math.floor(Math.random() * 256);
        img.data[i + 3] = 255;
      }
      ctx.putImageData(img, 0, 0);
      setTimeout(() => { window.__EGRET_PRINT_READY__ = true; }, 50);
    </script>
  </body></html>`,
  // Never sets either flag — the readiness wait must time out.
  "/never": `<!doctype html><html><body>
    <p>this page never signals readiness or failure</p>
  </body></html>`,
  // Signals failure — must produce a non-2xx with no image bytes.
  "/failed": `<!doctype html><html><body>
    <script>window.__EGRET_PRINT_FAILED__ = true;</script>
  </body></html>`,
  // Mirrors the real print page's structure (`frontend/src/routes/print/PrintObject.tsx`): a
  // `data-print-root` wrapper with `min-height: 100vh` — which always fills the viewport
  // regardless of its own content — wrapping two short blocks that together are much shorter
  // than the requested viewport height. A renderer that measured the wrapper (or
  // `document.documentElement.scrollHeight`) would report the full viewport; measuring the
  // wrapper's CHILDREN reports the true ~150px of content.
  "/short-content": `<!doctype html><html><body style="margin:0">
    <div data-print-root style="min-height:100vh;margin:0;padding:0;width:100%;background:#fff">
      <div style="height:40px;margin:0;padding:0;background:#111"></div>
      <div style="height:110px;margin:0;padding:0;background:#333"></div>
    </div>
    <script>window.__EGRET_PRINT_READY__ = true;</script>
  </body></html>`,
  // Same structure, but content taller than the requested viewport — the captured image must
  // grow to fit it rather than crop the bottom off.
  "/tall-content": `<!doctype html><html><body style="margin:0">
    <div data-print-root style="min-height:100vh;margin:0;padding:0;width:100%;background:#fff">
      <div style="height:1200px;margin:0;padding:0;background:#111"></div>
      <div style="height:1300px;margin:0;padding:0;background:#333"></div>
    </div>
    <script>window.__EGRET_PRINT_READY__ = true;</script>
  </body></html>`,
  // Reports the width it was laid out at before it signals readiness — the width a real chart
  // sizes its canvas to. `k` keeps one test's reports apart from another's.
  "/width": `<!doctype html><html><body style="margin:0">
    <div data-print-root><div style="height:200px;background:#111"></div></div>
    <script>
      const k = new URLSearchParams(location.search).get("k");
      fetch("/seen?k=" + k + "&w=" + window.innerWidth).then(() => {
        window.__EGRET_PRINT_READY__ = true;
      });
    </script>
  </body></html>`,
};

/** `/width` reports, by key: the innerWidth of every page that loaded with that key. */
const seenWidths = new Map();

let fixtureServer;
let fixtureBase;
let app; // default readiness timeout
let appBase;
let shortApp; // short readiness timeout, for the timeout test
let shortAppBase;

before(async () => {
  fixtureServer = createServer((req, res) => {
    const path = (req.url || "/").split("?")[0];
    if (path === "/seen") {
      const q = new URL(req.url, "http://fixture").searchParams;
      const k = q.get("k");
      seenWidths.set(k, [...(seenWidths.get(k) ?? []), Number(q.get("w"))]);
      res.writeHead(204);
      res.end();
      return;
    }
    const body = FIXTURES[path];
    if (!body) {
      res.writeHead(404, { "content-type": "text/plain" });
      res.end("not found");
      return;
    }
    res.writeHead(200, { "content-type": "text/html" });
    res.end(body);
  });
  await new Promise((resolve) => fixtureServer.listen(0, "127.0.0.1", resolve));
  fixtureBase = `http://127.0.0.1:${fixtureServer.address().port}`;

  app = createApp();
  await new Promise((resolve) => app.listen(0, "127.0.0.1", resolve));
  appBase = `http://127.0.0.1:${app.address().port}`;

  // Short timeout so the "never signals readiness" test stays fast.
  shortApp = createApp({ readyTimeoutMs: 500 });
  await new Promise((resolve) => shortApp.listen(0, "127.0.0.1", resolve));
  shortAppBase = `http://127.0.0.1:${shortApp.address().port}`;
});

after(async () => {
  await Promise.all([
    new Promise((resolve) => app.close(resolve)),
    new Promise((resolve) => shortApp.close(resolve)),
    new Promise((resolve) => fixtureServer.close(resolve)),
  ]);
  await closeBrowser();
});

async function postRender(base, payload) {
  const res = await fetch(`${base}/render`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(payload),
  });
  const body = Buffer.from(await res.arrayBuffer());
  return { status: res.status, body };
}

function isPngMagic(buf) {
  return (
    buf.length >= 8 &&
    buf[0] === 0x89 &&
    buf[1] === 0x50 && // P
    buf[2] === 0x4e && // N
    buf[3] === 0x47 // G
  );
}

function pngDimensions(buf) {
  // IHDR is always the first chunk: width @ bytes 16-19, height @ 20-23 (big-endian).
  return { width: buf.readUInt32BE(16), height: buf.readUInt32BE(20) };
}

test("a page that signals readiness returns real PNG bytes, not an icon-sized capture", async () => {
  const { status, body } = await postRender(appBase, { url: `${fixtureBase}/ready` });

  assert.equal(status, 200);
  assert.ok(isPngMagic(body), "response does not start with the PNG magic number");

  const { width, height } = pngDimensions(body);
  assert.ok(
    width > PNG_ICON_LIMIT && height > PNG_ICON_LIMIT,
    `PNG dimensions ${width}x${height} look like a grabbed UI icon, not rendered content`,
  );
  assert.ok(body.length > 4096, `PNG body of ${body.length} bytes is suspiciously small`);
});

test("a page that never signals readiness or failure times out with no image", async () => {
  const t0 = Date.now();
  const { status, body } = await postRender(shortAppBase, { url: `${fixtureBase}/never` });
  const elapsedMs = Date.now() - t0;

  assert.notEqual(status, 200);
  assert.ok(status >= 500 && status < 600, `expected a 5xx, got ${status}`);
  assert.ok(!isPngMagic(body), "a timeout must never return image bytes");

  const text = body.toString("utf8");
  assert.match(text.toLowerCase(), /timeout/, "error body should mention the timeout");

  // Load-bearing regression guard for the `page.waitForFunction(fn, undefined,
  // options)` arg-order fix: `shortApp` is configured with a 500ms readiness
  // timeout, so a correct implementation returns in well under a second. If
  // the `undefined` arg were ever dropped again, Playwright would silently
  // fall back to its own 30s default timeout instead of honoring
  // `readyTimeoutMs` — asserting on the status/message alone would NOT catch
  // that regression (it would still eventually time out, just 60x slower),
  // so this asserts wall-clock time too.
  assert.ok(
    elapsedMs < 5000,
    `timeout took ${elapsedMs}ms — expected well under 5000ms for a 500ms readyTimeoutMs; ` +
      `~30000ms would indicate the waitForFunction(fn, undefined, options) arg-order fix regressed`,
  );
});

test("a navigation failure never leaks the render token into the response body or logs", async () => {
  // A port with nothing listening reproduces a realistic navigation failure
  // (connection refused) without any dependency on the network.
  const probe = createServer();
  await new Promise((resolve) => probe.listen(0, "127.0.0.1", resolve));
  const deadPort = probe.address().port;
  await new Promise((resolve) => probe.close(resolve));

  const secretToken = "SECRET_RENDER_TOKEN_9f3a7c";
  const deadUrl = `http://127.0.0.1:${deadPort}/embed/print/chart/abc-123?t=${secretToken}`;

  // Playwright conventionally embeds the full navigated URL — token included
  // — in its own navigation-error message. Capture every console.log call
  // during this request so we can assert the token never reaches a log line,
  // not just the HTTP response.
  const originalLog = console.log;
  const loggedLines = [];
  console.log = (...args) => {
    loggedLines.push(args.map(String).join(" "));
  };

  let status, body;
  try {
    ({ status, body } = await postRender(appBase, { url: deadUrl }));
  } finally {
    console.log = originalLog;
  }

  assert.ok(status >= 400 && status < 600, `expected a non-2xx status, got ${status}`);
  assert.ok(!isPngMagic(body), "a navigation failure must never return image bytes");

  const responseText = body.toString("utf8");
  assert.ok(
    !responseText.includes(secretToken),
    `render token leaked into the response body: ${responseText}`,
  );
  assert.ok(
    !loggedLines.some((line) => line.includes(secretToken)),
    `render token leaked into a log line: ${JSON.stringify(loggedLines)}`,
  );
});

test("a page that reports __EGRET_PRINT_FAILED__ returns non-2xx with no image", async () => {
  const { status, body } = await postRender(appBase, { url: `${fixtureBase}/failed` });

  assert.ok(status < 200 || status >= 300, `expected non-2xx, got ${status}`);
  assert.ok(!isPngMagic(body), "a reported failure must never return image bytes");
});

test("a short page is captured at its content height, not the full requested viewport", async () => {
  const REQUESTED_WIDTH = 500;
  const REQUESTED_HEIGHT = 400;

  const { status, body } = await postRender(appBase, {
    url: `${fixtureBase}/short-content`,
    width: REQUESTED_WIDTH,
    height: REQUESTED_HEIGHT,
  });

  assert.equal(status, 200);
  const { width, height } = pngDimensions(body);
  assert.equal(width, REQUESTED_WIDTH, "requested width must stay authoritative");
  assert.ok(
    height < REQUESTED_HEIGHT,
    `short content (~150px) captured at height ${height} — expected it to be well under the ` +
      `${REQUESTED_HEIGHT}px requested viewport, not stretched to fill it (the white-band bug)`,
  );
  // Content is exactly two stacked blocks of 40px + 110px = 150px. Give a little slack for
  // devicePixelRatio/rounding, but this must be close to content, not close to the viewport.
  assert.ok(
    height >= 140 && height <= 170,
    `expected captured height close to the real ~150px content, got ${height}`,
  );
});

test("a tall page is captured in full, never cropped to the requested viewport", async () => {
  const REQUESTED_WIDTH = 500;
  const REQUESTED_HEIGHT = 400;

  const { status, body } = await postRender(appBase, {
    url: `${fixtureBase}/tall-content`,
    width: REQUESTED_WIDTH,
    height: REQUESTED_HEIGHT,
  });

  assert.equal(status, 200);
  const { width, height } = pngDimensions(body);
  assert.equal(width, REQUESTED_WIDTH, "requested width must stay authoritative");
  assert.ok(
    height > REQUESTED_HEIGHT,
    `tall content (2500px) captured at height ${height} — must exceed the requested ` +
      `${REQUESTED_HEIGHT}px viewport rather than being cropped to it`,
  );
  // Content is exactly two stacked blocks of 1200px + 1300px = 2500px.
  assert.ok(
    height >= 2490 && height <= 2510,
    `expected captured height close to the real ~2500px content, got ${height}`,
  );
});

test("a short page and a tall page rendered at the same requested size produce different heights", async () => {
  const size = { width: 500, height: 400 };
  const [shortResult, tallResult] = await Promise.all([
    postRender(appBase, { url: `${fixtureBase}/short-content`, ...size }),
    postRender(appBase, { url: `${fixtureBase}/tall-content`, ...size }),
  ]);

  assert.equal(shortResult.status, 200);
  assert.equal(tallResult.status, 200);
  const shortHeight = pngDimensions(shortResult.body).height;
  const tallHeight = pngDimensions(tallResult.body).height;
  assert.notEqual(
    shortHeight,
    tallHeight,
    "short and tall content rendered at the same requested viewport must not produce the same " +
      "captured height",
  );
});

test('format: "pdf" returns PDF bytes', async () => {
  const { status, body } = await postRender(appBase, {
    url: `${fixtureBase}/ready`,
    format: "pdf",
  });

  assert.equal(status, 200);
  assert.equal(body.toString("ascii", 0, 5), "%PDF-");
});

test("several urls come back as ONE pdf, not the first page only", async () => {
  // The multi-object path had no coverage at all, and shipped a
  // `withSlot is not defined` ReferenceError: `withSlot` is created inside
  // `createApp`, so a module-scope helper cannot reach it. Every single-URL
  // test passed straight through the bug.
  const { status, body } = await postRender(appBase, {
    urls: [`${fixtureBase}/ready`, `${fixtureBase}/ready`],
    format: "pdf",
  });

  assert.equal(status, 200);
  assert.equal(body.toString("ascii", 0, 5), "%PDF-");
  // Two sheets, not one. Chromium writes one `/Type /Page` per page, so
  // counting them is the difference between a real two-page document and the
  // first capture returned alone.
  // EXACTLY one sheet per object. ">= 2" would have passed the first
  // implementation, which spilled a tall capture onto extra sheets and cut it
  // wherever the page boundary landed — three sheets for two dashboards.
  const pages = body.toString("latin1").match(/\/Type\s*\/Page[^s]/g) || [];
  assert.equal(pages.length, 2, `expected exactly 2 pages, got ${pages.length}`);
});

test("a png request with several urls is refused rather than answered with one of them", async () => {
  // A PNG has no second page. Returning one image of several would let the
  // caller believe it had the whole report.
  const { status, body } = await postRender(appBase, {
    urls: [`${fixtureBase}/ready`, `${fixtureBase}/ready`],
    format: "png",
  });
  assert.equal(status, 200);
  // The single-image fallback is documented behaviour, so assert what it
  // actually is rather than pretending it errors.
  assert.equal(body.toString("ascii", 1, 4), "PNG");
});

/** The first page's size in CSS px (a PDF's MediaBox is in points, 72 to the inch; CSS has 96). */
function pdfPageSizePx(buf) {
  const m = buf.toString("latin1").match(/\/MediaBox\s*\[\s*0\s+0\s+([\d.]+)\s+([\d.]+)\s*\]/);
  assert.ok(m, "no MediaBox in the PDF");
  return { width: (Number(m[1]) * 96) / 72, height: (Number(m[2]) * 96) / 72 };
}

// KI-283: a PDF used to be laid out at the requested screen width (1200 px for a dashboard), so
// every chart drew its canvas that wide, and Chromium then shrank the whole page to fit the
// sheet: labels and axes came out at about two thirds of their size. The page must be laid out
// at the width of the sheet it is printed on, so the PDF prints it 1:1.
test("a PDF is laid out at the width of its own A4 sheet, not the requested screen width", async () => {
  const { status, body } = await postRender(appBase, {
    url: `${fixtureBase}/width?k=pdf-one`,
    width: 1200,
    height: 800,
    format: "pdf",
  });
  assert.equal(status, 200);
  const page = pdfPageSizePx(body);
  // Chromium's A4: 8.27 x 11.7 in, 595.92 x 842.88 pt.
  assert.ok(Math.abs(page.width - 794.56) < 0.1 && Math.abs(page.height - 1123.84) < 0.1, `not A4: ${JSON.stringify(page)}`);
  const [laidOut] = seenWidths.get("pdf-one") ?? [];
  assert.ok(laidOut <= page.width && laidOut > page.width - 1, `laid out at ${laidOut}px on a ${page.width}px sheet`);
});

test("several objects in one PDF are each captured at the width of the sheet they are placed on", async () => {
  const { status, body } = await postRender(appBase, {
    urls: [`${fixtureBase}/width?k=pdf-many`, `${fixtureBase}/width?k=pdf-many`],
    width: 1200,
    height: 800,
    format: "pdf",
  });
  assert.equal(status, 200);
  const page = pdfPageSizePx(body);
  const widths = seenWidths.get("pdf-many") ?? [];
  assert.equal(widths.length, 2);
  for (const w of widths) assert.ok(w <= page.width && w > page.width - 1, `captured at ${w}px for a ${page.width}px sheet`);
});

test("a PNG keeps the requested width: it is an image, not a sheet", async () => {
  const { status } = await postRender(appBase, { url: `${fixtureBase}/width?k=png`, width: 1200, height: 800 });
  assert.equal(status, 200);
  assert.deepEqual(seenWidths.get("png"), [1200]);
});
