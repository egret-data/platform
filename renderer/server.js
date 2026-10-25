// egret-renderer: screenshots (or PDFs) a chrome-free print page for the
// scheduled-reports pipeline. A backend service mints a single-use token and
// asks us to open `/embed/print/:kind/:id?t=<token>`; that page sets
// `window.__EGRET_PRINT_READY__ = true` once it has finished drawing, or
// `window.__EGRET_PRINT_FAILED__ = true` if it gave up. We wait for one of
// those and return image bytes — or, on timeout or a reported failure, we
// return NOTHING. A half-drawn chart in someone's inbox is worse than a
// missing one, because nobody double-checks a report that arrived.
//
// This service holds no credentials of its own: the only secret it ever sees
// is the token baked into the URL it's handed. Never log a full URL — only
// its origin and path.

import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const READY_TIMEOUT_MS = Number(process.env.RENDER_READY_TIMEOUT_MS || 20000);
const MAX_CONCURRENT = Number(process.env.RENDER_MAX_CONCURRENT || 2);
const MAX_BODY_BYTES = 1024 * 1024; // 1 MiB is generous for a {url,width,height,format,scale} body

// A page whose content is far taller than any real report should ever be gets capped here
// rather than captured at its full height — Chromium's memory, not CPU, is this pod's binding
// constraint, and an unbounded capture buffer for a runaway/malicious page is how you OOM it.
const MAX_CAPTURE_HEIGHT_PX = 20000;

// A PDF is printed on A4, and laid out at A4's width (KI-283). The print page draws each chart at
// the width of the window it is opened in; Chromium then prints that layout onto the sheet, and a
// layout wider than the sheet is shrunk to fit it. At the old 1200-1280 px screen width every
// label, axis and table cell came out at about two thirds of its size. Opening the page in a
// window exactly as wide as the sheet makes the layout and the paper agree, so nothing is scaled.
// Chromium's A4 sheet is 595.92 x 842.88 pt, i.e. 794.56 x 1123.84 CSS px (96 px to the inch);
// the window is floored to whole pixels so the layout is never wider than the sheet.
const A4_WIDTH_PX = 794;
const A4_HEIGHT_PX = 1123;
// Top and bottom only: the print page carries its own side padding, and a page break needs room
// above and below the card it lands on.
const PDF_OPTIONS = {
  format: "A4",
  printBackground: true,
  margin: { top: "10mm", bottom: "10mm", left: "0", right: "0" },
};

// A report covering more objects than this is almost certainly a mistake, and
// each one costs a full page load in a pod sized for two at a time.
const MAX_URLS = Number(process.env.RENDER_MAX_URLS || 20);

// One browser PROCESS for the life of the service — Chromium's memory, not
// CPU, is the binding constraint, so we never spawn more than one. Each
// request gets its own page (via a fresh context), bounded by the
// concurrency cap below.
let browserPromise = null;
function getBrowser() {
  if (!browserPromise) {
    browserPromise = chromium.launch({ headless: true });
  }
  return browserPromise;
}

export async function closeBrowser() {
  if (!browserPromise) return;
  const browser = await browserPromise;
  browserPromise = null;
  await browser.close().catch(() => {});
}

// Redact before logging: only the origin + path ever get written out, never
// the query string, which is where the render token lives.
function safeLocation(url) {
  try {
    const u = new URL(url);
    return `${u.origin}${u.pathname}`;
  } catch {
    return "(unparseable url)";
  }
}

// A tiny counting semaphore. `MAX_CONCURRENT` pages may be open across the
// whole process at once; everyone else queues in FIFO order.
function createLimiter(maxConcurrent) {
  let active = 0;
  const queue = [];
  return async function withSlot(fn) {
    if (active >= maxConcurrent) {
      await new Promise((resolve) => queue.push(resolve));
    }
    active++;
    try {
      return await fn();
    } finally {
      active--;
      const next = queue.shift();
      if (next) next();
    }
  };
}

// A common base so the route handler can recognise "a message WE constructed"
// versus "raw text from Playwright/Node that might still carry the render
// token". Every RenderError subclass below is built exclusively from fixed
// template strings plus, at most, `safeLocation(url)` — never from another
// error's own `.message`.
class RenderError extends Error {}
class RenderNavigationError extends RenderError {}
class RenderTimeoutError extends RenderError {}
class RenderFailedError extends RenderError {}
class RenderInternalError extends RenderError {}

async function renderOnce({ url, width, height, format, scale, readyTimeoutMs }) {
  let context;
  try {
    const browser = await getBrowser();
    // A PDF ignores the requested size: its page is the sheet (see `A4_WIDTH_PX`). The requested
    // width is an image's width, and only a PNG is an image.
    const pdf = format === "pdf";
    const viewportWidth = pdf ? A4_WIDTH_PX : Number.isFinite(width) && width > 0 ? Math.round(width) : 1280;
    const viewportHeight = pdf ? A4_HEIGHT_PX : Number.isFinite(height) && height > 0 ? Math.round(height) : 800;
    context = await browser.newContext({
      viewport: { width: viewportWidth, height: viewportHeight },
      deviceScaleFactor: Number.isFinite(scale) && scale > 0 ? scale : 1,
    });
    return await captureReadyPage(context, { url, format, readyTimeoutMs, viewportWidth });
  } catch (e) {
    if (e instanceof RenderError) throw e;
    // Anything we didn't explicitly classify below — a browser-launch
    // failure, a crashed page, some other Playwright internal error — might
    // still carry the raw URL: Playwright conventionally embeds the full
    // navigated URL (query string and all, i.e. the render token) in its own
    // error messages. Never let that raw text reach a log line or a response
    // body; replace it with a message we build ourselves, capped at the
    // URL's origin + path.
    throw new RenderInternalError(`unexpected render error (${safeLocation(url)})`);
  } finally {
    if (context) await context.close().catch(() => {});
  }
}

// Measures the print page's real content height, AFTER readiness, so the caller can size the
// capture to it instead of the fixed requested viewport — that fixed viewport is what leaves a
// white band below short content (or, worse, silently crops tall content).
//
// `document.documentElement.scrollHeight` is NOT used: the print page's own wrapper
// (`frontend/src/routes/print/PrintObject.tsx`) sets `min-height: 100vh` so it always fills the
// viewport, even when its own content is much shorter — measuring the document root (or that
// wrapper itself) would just report the viewport height back to us, defeating the whole point of
// this measurement. Instead this looks at the wrapper's DIRECT CHILDREN (the title and each
// card): those size themselves off their own content, never off the wrapper's `min-height`, so
// the tallest child's bottom edge is the true content height regardless of how the wrapper
// stretches. `[data-print-root]` names that wrapper explicitly; pages without it (e.g. this
// service's own test fixtures) fall back to `document.body`.
async function measureContentHeight(page) {
  const height = await page.evaluate(() => {
    const root = document.querySelector("[data-print-root]") || document.body;
    let maxBottom = 0;
    for (const el of root.children) {
      const bottom = el.getBoundingClientRect().bottom;
      if (bottom > maxBottom) maxBottom = bottom;
    }
    // Only reached if `root` rendered no children at all — fall back to its own scrollHeight
    // rather than report zero.
    if (maxBottom <= 0) maxBottom = root.scrollHeight;
    return maxBottom;
  });
  return Math.ceil(height);
}

async function captureReadyPage(context, { url, format, readyTimeoutMs, viewportWidth }) {
  let page;
  try {
    page = await context.newPage();
    await page.goto(url, { waitUntil: "domcontentloaded" });
  } catch (e) {
    // e.g. DNS failure, connection refused, or a navigation timeout — all
    // real production failure modes, and Playwright's own error text for
    // every one of them includes the full URL it tried to navigate to.
    throw new RenderNavigationError(`failed to load the print page at ${safeLocation(url)}`);
  }

  try {
    // Waits for the page's OWN readiness flag. A timeout here is a FAILED
    // render — we must not fall through and screenshot a half-drawn page.
    //
    // NOTE: `page.waitForFunction`'s real signature is
    // `(pageFunction, arg, options)` — passing `{ timeout }` as the SECOND
    // positional argument (a common mistake) is silently treated as `arg`,
    // not `options`, and the call falls back to the 30s default timeout
    // instead of `readyTimeoutMs`. The explicit `undefined` here is load
    // bearing (see the elapsed-time regression test in test/render.test.js).
    await page.waitForFunction(
      () => window.__EGRET_PRINT_READY__ === true || window.__EGRET_PRINT_FAILED__ === true,
      undefined,
      { timeout: readyTimeoutMs },
    );
  } catch (e) {
    if (e?.name === "TimeoutError") {
      throw new RenderTimeoutError(
        `print page at ${safeLocation(url)} did not signal readiness within ${readyTimeoutMs}ms (timeout)`,
      );
    }
    throw new RenderInternalError(`error while waiting for readiness on ${safeLocation(url)}`);
  }

  const failed = await page.evaluate(() => window.__EGRET_PRINT_FAILED__ === true);
  if (failed) {
    throw new RenderFailedError(`the print page at ${safeLocation(url)} reported a failure`);
  }

  if (format === "pdf") {
    // Deliberately NOT given the same content-height treatment as the PNG path below: `page.pdf`
    // lays out onto paper-sized pages via print CSS and paginates automatically (it has never
    // used the screenshot viewport as an image canvas), so it cannot exhibit "content shorter
    // than a fixed capture height" — there is no equivalent white band to fix here. The sheet is
    // the one the window was sized to, so the layout prints 1:1.
    const buffer = await page.pdf(PDF_OPTIONS);
    return { buffer, contentType: "application/pdf" };
  }

  // Measurement happens strictly AFTER the readiness wait and failure check above — never
  // instead of it — so a still-drawing or genuinely stuck page can never reach this point.
  const measuredHeight = await measureContentHeight(page);
  // Clamp: never capture a zero/negative height (an unmeasurable or truly empty page), and cap
  // at MAX_CAPTURE_HEIGHT_PX so a runaway page can't force an enormous capture buffer.
  const captureHeight = Math.min(Math.max(measuredHeight, 1), MAX_CAPTURE_HEIGHT_PX);
  // Resize (rather than pass a `clip` taller than the current viewport) so the capture is just
  // "screenshot the whole viewport" — no reliance on Playwright's clip-beyond-viewport behavior.
  // Only the height changes; the requested width stays authoritative.
  await page.setViewportSize({ width: viewportWidth, height: captureHeight });
  const buffer = await page.screenshot({ type: "png" });
  return { buffer, contentType: "image/png" };
}

/**
 * Several print pages, one document.
 *
 * Each URL is captured as a PNG through the SAME single-page path as a
 * one-object report — so multi-object rendering cannot drift from single-object
 * rendering — and the images are then laid into an HTML page, one per printed
 * sheet, which Chromium turns into the final PDF. Doing it this way keeps the
 * renderer dependency-free: merging real PDFs would need a PDF library, while
 * Chromium is already here and already paginates.
 *
 * Captures run SEQUENTIALLY through the caller's semaphore — acquired and
 * released once per page, never held across another acquire, so two concurrent
 * multi-object reports cannot deadlock each other.
 * Rendering twenty dashboards at once is how a 512Mi pod dies, and the report
 * is not latency-sensitive.
 *
 * `png` with several URLs returns the FIRST image only — a PNG has no second
 * page, and silently returning one of twenty would be a lie. The caller
 * (`runner.rs`) forces `pdf` for multi-target reports; this branch exists so a
 * hand-made request cannot produce something misleading.
 */
async function renderMany({ urls, width, height, format, scale, readyTimeoutMs, withSlot }) {
  // Each object is captured as wide as the sheet it is placed on, for the same reason a
  // single-object PDF is laid out at it: an image wider than the sheet is shrunk to fit (KI-283).
  // A tall object is still scaled down to one sheet, by design (see below).
  const pdf = format === "pdf";
  const shotWidth = pdf ? A4_WIDTH_PX : width;
  const shotHeight = pdf ? A4_HEIGHT_PX : height;
  const shots = [];
  for (const url of urls) {
    const { buffer } = await withSlot(() =>
      renderOnce({ url, width: shotWidth, height: shotHeight, format: "png", scale, readyTimeoutMs }),
    );
    shots.push(buffer);
  }
  if (!pdf) {
    return { buffer: shots[0], contentType: "image/png" };
  }

  const pages = shots
    .map(
      (b) =>
        `<div class="page"><img src="data:image/png;base64,${b.toString("base64")}"></div>`,
    )
    .join("");
  // ONE sheet per object, scaled to fit — the same contract as Superset's
  // `build_pdf_from_screenshots`. Without the height bound a tall dashboard
  // simply overflows onto the next sheet and is cut wherever the page boundary
  // happens to land, which can slice a chart in half; a first attempt produced
  // three sheets for two dashboards exactly that way.
  //
  // `page-break-after` is skipped on the last sheet; applying it to all of them
  // is the classic trailing-blank-page artefact.
  const doc =
    `<!doctype html><html><head><meta charset="utf-8"><style>` +
    `@page{margin:0}html,body{margin:0;padding:0}` +
    `.page{page-break-after:always;height:100vh;display:flex;` +
    `align-items:center;justify-content:center;overflow:hidden}` +
    `.page:last-child{page-break-after:auto}` +
    `img{max-width:100%;max-height:100%;object-fit:contain}` +
    `</style></head><body>${pages}</body></html>`;

  return await withSlot(async () => {
    const b = await getBrowser();
    const context = await b.newContext({
      viewport: { width: A4_WIDTH_PX, height: A4_HEIGHT_PX },
    });
    try {
      const page = await context.newPage();
      // `setContent` with no network waits: the images are data URIs, so there
      // is nothing to fetch and `networkidle` would just add a timeout.
      await page.setContent(doc, { waitUntil: "load" });
      // Full-bleed sheets (`@page{margin:0}` above): each capture carries the print page's own
      // padding, so the margin is already in the image.
      const buffer = await page.pdf({ format: "A4", printBackground: true });
      return { buffer, contentType: "application/pdf" };
    } finally {
      await context.close();
    }
  });
}

function readBody(req, maxBytes) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let total = 0;
    req.on("data", (chunk) => {
      total += chunk.length;
      if (total > maxBytes) {
        reject(new Error("request body too large"));
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on("end", () => resolve(Buffer.concat(chunks)));
    req.on("error", reject);
  });
}

function sendJson(res, status, body) {
  const payload = Buffer.from(JSON.stringify(body));
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": payload.length,
  });
  res.end(payload);
}

/**
 * Build the http.Server. Options let tests override the readiness timeout /
 * concurrency cap without racing module-level env-var evaluation.
 */
export function createApp({
  readyTimeoutMs = READY_TIMEOUT_MS,
  maxConcurrent = MAX_CONCURRENT,
} = {}) {
  const withSlot = createLimiter(maxConcurrent);

  return createServer(async (req, res) => {
    if (req.method === "GET" && req.url === "/healthz") {
      res.writeHead(200, { "content-type": "text/plain" });
      res.end("ok");
      return;
    }

    if (req.method === "POST" && req.url === "/render") {
      let body;
      try {
        const raw = await readBody(req, MAX_BODY_BYTES);
        body = JSON.parse(raw.toString("utf8"));
      } catch {
        // Deliberately not echoing the parse error's own message: the raw
        // request body is exactly `{url, ...}`, i.e. it may itself contain
        // the render token, and some JSON.parse error messages quote a
        // snippet of the offending input.
        sendJson(res, 400, { error: "invalid request body: could not parse as JSON" });
        return;
      }

      const { url, urls, width, height, format, scale } = body ?? {};
      // `urls` is the multi-object form: several print pages, one document.
      // `url` stays the single-object form every existing caller sends.
      const list = Array.isArray(urls) && urls.length > 0 ? urls : url ? [url] : [];
      if (list.length === 0 || list.some((u) => typeof u !== "string" || u.length === 0)) {
        sendJson(res, 400, { error: "missing required field: url or urls" });
        return;
      }
      if (list.length > MAX_URLS) {
        sendJson(res, 400, { error: `too many urls: at most ${MAX_URLS}` });
        return;
      }

      console.log(
        `[render] request ${safeLocation(list[0])} count=${list.length} format=${format || "png"}`,
      );

      try {
        const { buffer, contentType } =
          list.length === 1
            ? await withSlot(() =>
                renderOnce({ url: list[0], width, height, format, scale, readyTimeoutMs }),
              )
            : await renderMany({
                urls: list,
                width,
                height,
                format,
                scale,
                readyTimeoutMs,
                // Passed in, not captured: the semaphore belongs to the app
                // instance (`createApp`), and a module-scope function cannot
                // see it. Reaching for it from here was a ReferenceError that
                // only fired on the multi-URL path.
                withSlot,
              });
        res.writeHead(200, {
          "content-type": contentType,
          "content-length": buffer.length,
        });
        res.end(buffer);
      } catch (e) {
        const status = e instanceof RenderTimeoutError ? 504 : 502;
        console.log(
          `[render] failed ${safeLocation(list[0])} status=${status} reason=${e.message}`,
        );
        sendJson(res, status, { error: e.message });
      }
      return;
    }

    sendJson(res, 404, { error: "not found" });
  });
}

// Only auto-start when run directly (`node server.js`) — tests import
// `createApp` and drive their own ephemeral-port instances instead.
const isMain = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (isMain) {
  const port = Number(process.env.PORT || 8080);
  const app = createApp();
  app.listen(port, () => {
    console.log(`egret-renderer listening on :${port}`);
  });

  const shutdown = (signal) => {
    console.log(`[render] ${signal} received, shutting down`);
    app.close(() => {
      closeBrowser().finally(() => process.exit(0));
    });
    // Don't hang forever waiting for in-flight renders.
    setTimeout(() => process.exit(0), 10_000).unref();
  };
  process.on("SIGTERM", () => shutdown("SIGTERM"));
  process.on("SIGINT", () => shutdown("SIGINT"));
}
