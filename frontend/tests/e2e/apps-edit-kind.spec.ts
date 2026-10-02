import https from "node:https";
import { test as base, expect, type Page, type FrameLocator } from "@playwright/test";
import { loginLocalAdmin, csrf, firstWorkspace } from "./helpers";

/**
 * Data Apps, edit mode, against the in-cluster kind deploy served over HTTPS (spec §10, P4e).
 * Opt-in: EGRET_E2E_APPS=1, EGRET_E2E_BASE_URL=https://<egret host>, EGRET_E2E_INSECURE_TLS=1.
 *
 * What it proves, with a real browser and a real Kubernetes pod:
 *  - an author opens an app in egret and marimo renders from the APPS origin, in a sandboxed iframe;
 *  - JavaScript an author wrote into the app cannot read egret's page, cookies or responses, and
 *    cannot complete a state change on egret (no-cors POST, header-bearing POST);
 *  - a cookie that JavaScript tosses at egret from the sibling origin does not satisfy egret's CSRF
 *    check — `__Host-egret_csrf` is the only one egret reads.
 */
const ENABLED = process.env.EGRET_E2E_APPS === "1";
const APPS_ORIGIN = process.env.EGRET_E2E_APPS_ORIGIN ?? "https://apps.verify.localtest.me";
const EGRET_ORIGIN = process.env.EGRET_E2E_BASE_URL ?? "https://verify.localtest.me";

function hostileSource(slug: string): string {
  const esm = `
export default {
  async render({ el }) {
    const r = {};
    const out = (k, v) => { r[k] = v; el.textContent = "egret-probe:" + JSON.stringify(r); };
    out("started", true);
    try { void parent.document.title; out("parentDocument", "READABLE"); }
    catch (e) { out("parentDocument", e.name); }
    out("cookie", document.cookie);
    try {
      const res = await fetch("${EGRET_ORIGIN}/api/auth/me", { credentials: "include" });
      out("readMe", "status " + res.status);
    } catch (e) { out("readMe", e.name); }
    try {
      await fetch("${EGRET_ORIGIN}/api/ws/${slug}/apps", {
        method: "POST", mode: "no-cors", credentials: "include",
        headers: { "Content-Type": "text/plain" }, body: JSON.stringify({ name: "evil-nocors" }),
      });
      out("noCorsPost", "sent");
    } catch (e) { out("noCorsPost", e.name); }
    try {
      await fetch("${EGRET_ORIGIN}/api/ws/${slug}/apps", {
        method: "POST", credentials: "include",
        headers: { "Content-Type": "application/json", "X-Egret-CSRF": "evil" },
        body: JSON.stringify({ name: "evil-header" }),
      });
      out("headerPost", "sent");
    } catch (e) { out("headerPost", e.name); }
    document.cookie = "egret_csrf=evil; Domain=${new URL(EGRET_ORIGIN).hostname}; Path=/; Secure; SameSite=None";
    document.cookie = "__Host-egret_csrf=evil; Domain=${new URL(EGRET_ORIGIN).hostname}; Path=/; Secure; SameSite=None";
    out("tossed", "attempted");
  },
};`;
  return [
    "import marimo",
    "",
    "app = marimo.App()",
    "",
    "",
    "@app.cell",
    "def _():",
    "    import anywidget",
    "    import marimo as mo",
    "",
    "    class Probe(anywidget.AnyWidget):",
    `        _esm = ${JSON.stringify(esm)}`,
    "",
    "    widget = mo.ui.anywidget(Probe())",
    "    widget",
    "    return",
    "",
    "",
    'if __name__ == "__main__":',
    "    app.run()",
    "",
  ].join("\n");
}

async function api(page: Page, method: "get" | "post" | "put", url: string, data?: unknown, extra: Record<string, string> = {}) {
  const h = { ...(await csrf(page.context())), ...extra };
  return page.request[method](url, method === "get" ? { headers: h } : { headers: h, data });
}

const STATE = "test-results/.apps-kind-admin.json";

// Each test gets its OWN browser context built from the one saved login. Done as a fixture, not
// `test.use({ storageState })`, because that is read before `beforeAll` has written the file.
const test = base.extend({
  context: async ({ browser }, use) => {
    const context = await browser.newContext({ storageState: STATE, ignoreHTTPSErrors: true });
    await use(context);
    await context.close();
  },
  page: async ({ context }, use) => {
    await use(await context.newPage());
  },
});

test.describe("data apps — edit mode on kind", () => {
  test.skip(!ENABLED, "opt-in: EGRET_E2E_APPS=1 against the apps-enabled in-cluster kind deploy");
  test.setTimeout(240_000);

  // ONE login for the whole file: the login endpoint is rate limited (5 per 5 minutes), and every
  // test below used to sign in on its own. Each test still gets its own browser context from the
  // saved state, so a cookie one test tosses into its jar is not in the next test's.
  test.beforeAll(async ({ browser }) => {
    const context = await browser.newContext({ ignoreHTTPSErrors: true });
    await loginLocalAdmin(await context.newPage());
    await context.storageState({ path: STATE });
    await context.close();
  });

  test("an author's hostile app cannot read egret, change egret, or toss a cookie egret believes", async ({ page, context }) => {
    const ws = await firstWorkspace(page.request);

    // egret really issued the __Host- names (and not the plain ones) over HTTPS
    const names = (await context.cookies()).map((c) => c.name);
    expect(names, "cookies after login").toEqual(expect.arrayContaining(["__Host-egret_session", "__Host-egret_csrf"]));
    expect(names).not.toContain("egret_session");
    expect(names).not.toContain("egret_csrf");

    // an app whose draft is hostile JavaScript
    const created = await api(page, "post", `/api/ws/${ws.slug}/apps`, { name: `probe-${Date.now()}` });
    expect(created.status(), await created.text()).toBe(201);
    const app = await created.json();
    const draft = await (await api(page, "get", `/api/ws/${ws.slug}/apps/${app.id}/draft`)).json();
    const saved = await api(page, "put", `/api/ws/${ws.slug}/apps/${app.id}/draft`, { source: hostileSource(ws.slug) }, { "If-Match": `"${draft.revision}"` });
    expect(saved.status(), await saved.text()).toBe(200);

    // the Apps list shows it, as a draft, and Edit opens the editor
    await page.goto(`/explore/${ws.slug}/apps`);
    await expect(page.getByRole("row", { name: new RegExp(app.name) })).toContainText("Draft only");
    await page.getByRole("row", { name: new RegExp(app.name) }).getByRole("link", { name: /Edit/ }).click();
    await page.waitForURL(new RegExp(`/apps/${app.id}/edit`));

    // the iframe is on the APPS origin and sandboxed
    const iframe = page.locator('iframe[title$="editor"]');
    await expect(iframe).toBeAttached({ timeout: 120_000 });
    expect(await iframe.getAttribute("sandbox")).toBe("allow-scripts allow-same-origin allow-forms allow-downloads");
    const frame = page.frameLocator('iframe[title$="editor"]');

    // marimo's editor is up: the draft's source is in a cell. Edit mode does not run cells by itself,
    // so run this one the way an author would.
    const editor = frame.locator(".cm-content").first();
    await expect(editor).toBeVisible({ timeout: 180_000 });
    await expect(editor).toContainText("class Probe");
    await editor.click();
    await page.keyboard.press("Shift+Enter");

    // The widget's OUTPUT, not the cell's source (which also contains the string "egret-probe:"):
    // only the output has the JSON right after the colon.
    const probe = frame.locator("text=/egret-probe:\\{/");
    await expect(probe.first()).toContainText('"tossed":"attempted"', { timeout: 120_000 });
    const results = JSON.parse((await probe.first().innerText()).replace(/^egret-probe:/, ""));
    const frameUrl = page.frames().find((f) => f.url().startsWith(APPS_ORIGIN))?.url();
    expect(frameUrl, "a frame on the apps origin").toBeTruthy();

    // 1. it cannot read the parent egret page
    expect(results.parentDocument).toBe("SecurityError");
    // 2. no egret cookie is visible to it, and it cannot read an egret response
    expect(results.cookie).not.toMatch(/egret_session|__Host-/);
    expect(results.readMe).toBe("TypeError");
    // 3. the header-bearing POST dies at the CORS preflight
    expect(results.headerPost).toBe("TypeError");

    // 4. neither POST changed anything on egret
    const list = await (await api(page, "get", `/api/ws/${ws.slug}/apps`)).json();
    const evil = list.filter((a: { name: string }) => a.name.startsWith("evil-"));
    expect(evil, "an app created from inside an app").toEqual([]);

    // 5. a cookie tossed from the sibling origin is in the browser, but does not satisfy egret's CSRF
    const jar = await context.cookies();
    const tossedPlain = jar.find((c) => c.name === "egret_csrf");
    expect(tossedPlain?.value, "the plain-name cookie was tossed into the jar").toBe("evil");
    const hostCsrf = jar.find((c) => c.name === "__Host-egret_csrf");
    expect(hostCsrf?.value, "the __Host- cookie was NOT overwritten (the browser refuses a Domain on it)").not.toBe("evil");
    const forged = await page.request.post(`/api/ws/${ws.slug}/apps`, {
      headers: { "X-Egret-CSRF": "evil", "Content-Type": "application/json" },
      data: { name: "evil-forged" },
    });
    expect(forged.status(), "header matching the TOSSED cookie").toBe(403);
    const genuine = await page.request.post(`/api/ws/${ws.slug}/apps`, {
      headers: { "X-Egret-CSRF": hostCsrf!.value, "Content-Type": "application/json" },
      data: { name: "legit-after-toss" },
    });
    expect(genuine.status(), await genuine.text()).toBe(201);
  });

  // ── helpers for the cases below ────────────────────────────────────────────

  const SIMPLE = (body: string) =>
    ["import marimo", "", "app = marimo.App()", "", "", "@app.cell", "def _():", ...body.split("\n").map((l) => "    " + l), "    return", "", "", 'if __name__ == "__main__":', "    app.run()", ""].join("\n");

  async function newApp(page: Page, slug: string, source: string) {
    const created = await api(page, "post", `/api/ws/${slug}/apps`, { name: `e2e-${Date.now()}-${Math.floor(Math.random() * 1e4)}` });
    expect(created.status(), await created.text()).toBe(201);
    const app = await created.json();
    const draft = await (await api(page, "get", `/api/ws/${slug}/apps/${app.id}/draft`)).json();
    const saved = await api(page, "put", `/api/ws/${slug}/apps/${app.id}/draft`, { source }, { "If-Match": `"${draft.revision}"` });
    expect(saved.status(), await saved.text()).toBe(200);
    return { id: app.id as string, name: app.name as string, revision: (await saved.json()).revision as number };
  }

  async function openEditor(page: Page, slug: string, id: string): Promise<FrameLocator> {
    await page.goto(`/explore/${slug}/apps/${id}/edit`);
    await expect(page.locator('iframe[title$="editor"]')).toBeAttached({ timeout: 120_000 });
    const frame = page.frameLocator('iframe[title$="editor"]');
    await expect(frame.locator(".cm-content").first()).toBeVisible({ timeout: 180_000 });
    return frame;
  }

  async function runFirstCell(page: Page, frame: FrameLocator) {
    await frame.locator(".cm-content").first().click();
    await page.keyboard.press("Shift+Enter");
  }

  test("a cell asks egret for its databases through the real pod, as the author", async ({ page }) => {
    const ws = await firstWorkspace(page.request);
    const app = await newApp(page, ws.slug, SIMPLE('import marimo as mo\nimport egret_marimo\nmo.md(f"databases-probe:{egret_marimo.databases()!r}")'));
    const frame = await openEditor(page, ws.slug, app.id);
    await runFirstCell(page, frame);
    // pod -> egret (callback URL) -> gateway, with the per-session token the proxy injected
    await expect(frame.locator("text=/databases-probe:\\[/").first()).toBeVisible({ timeout: 120_000 });
  });

  test("a save over a draft someone else changed is refused in marimo itself, and a reload recovers", async ({ page }) => {
    const ws = await firstWorkspace(page.request);
    const app = await newApp(page, ws.slug, SIMPLE("x = 1"));
    const frame = await openEditor(page, ws.slug, app.id);

    // another author saves the draft while this editor holds the older revision
    const other = await api(page, "put", `/api/ws/${ws.slug}/apps/${app.id}/draft`, { source: SIMPLE("x = 'from the other author'") }, { "If-Match": `"${app.revision}"` });
    expect(other.status(), await other.text()).toBe(200);

    // this editor edits and saves
    await frame.locator(".cm-content").first().click();
    await page.keyboard.press("End");
    await page.keyboard.type("  # mine");
    await page.keyboard.press("ControlOrMeta+s");
    await expect(frame.locator("text=/saved by someone else/i").first()).toBeVisible({ timeout: 60_000 });

    // nothing of this editor's was written: the draft is still the other author's
    const draft = await (await api(page, "get", `/api/ws/${ws.slug}/apps/${app.id}/draft`)).json();
    expect(draft.source).toContain("from the other author");
    expect(draft.source).not.toContain("# mine");

    // Reload editor opens a NEW session at the latest revision
    await page.getByRole("button", { name: /Reload editor/ }).click();
    await expect(page.frameLocator('iframe[title$="editor"]').locator(".cm-content").first()).toContainText("from the other author", { timeout: 180_000 });
  });

  test("ambiguous spellings of a blocked route never reach marimo, through the real ingress", async ({ page, context }) => {
    const ws = await firstWorkspace(page.request);
    const app = await newApp(page, ws.slug, SIMPLE("x = 1"));
    await openEditor(page, ws.slug, app.id);
    const grant = (await context.cookies()).find((c) => c.name === "__Secure-egret_apps_grant" && c.domain.includes("apps."))!;
    const ref = new URL(page.frames().find((f) => f.url().startsWith(APPS_ORIGIN))!.url()).pathname.split("/")[2];
    const host = new URL(APPS_ORIGIN).hostname;

    // Node's https does not normalise the request target, unlike fetch or curl-without-flags
    const raw = (target: string) =>
      new Promise<number>((resolve, reject) => {
        const req = https.request({ host, port: 443, path: target, method: "GET", rejectUnauthorized: false, headers: { cookie: `${grant.name}=${grant.value}`, host } }, (res) => {
          res.resume();
          resolve(res.statusCode ?? 0);
        });
        req.on("error", reject);
        req.end();
      });

    const base = `/e/${ref}`;
    const blocked = ["api/packages/list", "api/files/list_files", "api/secrets/keys"];
    const variants = (b: string) => [
      `${base}/${b}`,
      `${base}/../${b}`,
      `${base}/%2e%2e/${b}`,
      `${base}/..%2f${b}`,
      `${base}/./${b}`,
      `${base}//${b}`,
      `${base}%5c${b}`,
      `${base}/%252e%252e/${b}`,
    ];
    const seen: Record<string, number> = {};
    for (const b of blocked) for (const t of variants(b)) seen[t] = await raw(t);
    // never marimo's own answer to a blocked route: 200 (listed packages) is the failure
    for (const [t, status] of Object.entries(seen)) {
      expect([400, 403, 404], `${t} -> ${status}`).toContain(status);
    }
    // and a legitimate route in the same app still works, so the above is not "everything is refused"
    expect(await raw(`${base}/api/version`)).toBe(200);
  });

  // LAST on purpose: logging out ends the session every test above shared.
  test("logging out ends the apps-origin session, not just egret's", async ({ page, context }) => {
    const ws = await firstWorkspace(page.request);
    const app = await newApp(page, ws.slug, SIMPLE("x = 1"));
    await openEditor(page, ws.slug, app.id);

    const grant = (await context.cookies()).find((c) => c.name === "__Secure-egret_apps_grant" && c.domain.includes("apps."));
    expect(grant, "the apps-origin cookie exists, HttpOnly").toBeTruthy();
    expect(grant!.httpOnly).toBe(true);
    expect(grant!.secure).toBe(true);
    const ref = new URL(page.frames().find((f) => f.url().startsWith(APPS_ORIGIN))!.url()).pathname.split("/")[2];
    const probe = () => context.request.get(`${APPS_ORIGIN}/e/${ref}/api/version`, { headers: { cookie: `${grant!.name}=${grant!.value}` } });
    expect((await probe()).status(), "before logout").toBe(200);

    const out = await api(page, "post", "/api/auth/logout", {});
    expect(out.status()).toBe(200);
    expect((await probe()).status(), "the same cookie after logout").toBe(401);
  });
});
