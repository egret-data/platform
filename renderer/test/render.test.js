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
};

let fixtureServer;
let fixtureBase;
let app; // default readiness timeout
let appBase;
let shortApp; // short readiness timeout, for the timeout test
let shortAppBase;

before(async () => {
  fixtureServer = createServer((req, res) => {
    const path = (req.url || "/").split("?")[0];
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
  const { status, body } = await postRender(shortAppBase, { url: `${fixtureBase}/never` });

  assert.notEqual(status, 200);
  assert.ok(status >= 500 && status < 600, `expected a 5xx, got ${status}`);
  assert.ok(!isPngMagic(body), "a timeout must never return image bytes");

  const text = body.toString("utf8");
  assert.match(text.toLowerCase(), /timeout/, "error body should mention the timeout");
});

test("a page that reports __EGRET_PRINT_FAILED__ returns non-2xx with no image", async () => {
  const { status, body } = await postRender(appBase, { url: `${fixtureBase}/failed` });

  assert.ok(status < 200 || status >= 300, `expected non-2xx, got ${status}`);
  assert.ok(!isPngMagic(body), "a reported failure must never return image bytes");
});

test('format: "pdf" returns PDF bytes', async () => {
  const { status, body } = await postRender(appBase, {
    url: `${fixtureBase}/ready`,
    format: "pdf",
  });

  assert.equal(status, 200);
  assert.equal(body.toString("ascii", 0, 5), "%PDF-");
});
