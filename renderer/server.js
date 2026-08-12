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
    context = await browser.newContext({
      viewport: {
        width: Number.isFinite(width) && width > 0 ? Math.round(width) : 1280,
        height: Number.isFinite(height) && height > 0 ? Math.round(height) : 800,
      },
      deviceScaleFactor: Number.isFinite(scale) && scale > 0 ? scale : 1,
    });
    return await captureReadyPage(context, { url, format, readyTimeoutMs });
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

async function captureReadyPage(context, { url, format, readyTimeoutMs }) {
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
    const buffer = await page.pdf({ printBackground: true });
    return { buffer, contentType: "application/pdf" };
  }
  const buffer = await page.screenshot({ type: "png" });
  return { buffer, contentType: "image/png" };
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

      const { url, width, height, format, scale } = body ?? {};
      if (typeof url !== "string" || url.length === 0) {
        sendJson(res, 400, { error: "missing required field: url" });
        return;
      }

      console.log(`[render] request ${safeLocation(url)} format=${format || "png"}`);

      try {
        const { buffer, contentType } = await withSlot(() =>
          renderOnce({ url, width, height, format, scale, readyTimeoutMs }),
        );
        res.writeHead(200, {
          "content-type": contentType,
          "content-length": buffer.length,
        });
        res.end(buffer);
      } catch (e) {
        const status = e instanceof RenderTimeoutError ? 504 : 502;
        console.log(
          `[render] failed ${safeLocation(url)} status=${status} reason=${e.message}`,
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
