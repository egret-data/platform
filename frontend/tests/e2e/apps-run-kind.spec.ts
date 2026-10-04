import { execFileSync } from "node:child_process";
import { test as base, expect, type Browser, type BrowserContext, type Page } from "@playwright/test";
import { addWorkspaceMember, csrf, firstWorkspace, loginLocalAdmin, postJson } from "./helpers";

/**
 * Data Apps, run mode, against the in-cluster kind deploy (spec §10, P5e) — the structural proof
 * that D24 holds in production, not just in the fake-pod proxy tests: two viewers of the SAME
 * published app land in two DIFFERENT pods (never one shared pod), an abandoned viewer's pod is
 * reaped while an active one's is not, and a third viewer does not disturb an existing one's pod.
 *
 * Opt-in: EGRET_E2E_APPS=1, EGRET_E2E_BASE_URL=https://<egret host>, EGRET_E2E_INSECURE_TLS=1.
 * Deploy with a short run idle so the reap step does not need a 5-minute wait:
 *   just deploy-kind-apps
 *   helm upgrade egret-verify deploy/helm/egret -n egret-verify --reuse-values \
 *     --set apps.pod.run.idleSecs=30
 *
 * The published app's source prints its own hostname — no SQL/engine dependency, on purpose:
 * D24's claim is about pod separation, not query permissions (already covered live by
 * apps-edit-kind.spec.ts's "real pod API" test), so this does not need a provisioned database.
 * Kubernetes sets a pod's hostname to the pod's own name, which is what makes this a real
 * cross-pod fingerprint — `os.getpid()` is NOT: each container gets its own PID namespace, so
 * "the first process in the container" is a small number like 11 in every pod alike, matching
 * between completely different pods and proving nothing (found live, first attempt at this test).
 */
const ENABLED = process.env.EGRET_E2E_APPS === "1";
const APPS_NAMESPACE = process.env.EGRET_E2E_APPS_NAMESPACE ?? "egret-verify-apps";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;
const STATE = "test-results/.apps-run-kind-admin.json";

function runPodNames(): string[] {
  const out = execFileSync(
    "kubectl",
    ["-n", APPS_NAMESPACE, "get", "pods", "-l", "egret.io/kind=run", "-o", "jsonpath={.items[*].metadata.name}"],
    { encoding: "utf8" },
  );
  return out.split(/\s+/).filter(Boolean);
}

const IDENTITY_APP = [
  "import marimo",
  "",
  "app = marimo.App()",
  "",
  "",
  "@app.cell",
  "def _():",
  "    import marimo as mo",
  "    import socket",
  '    mo.md(f"host={socket.gethostname()}")',
  "    return",
  "",
  "",
  'if __name__ == "__main__":',
  "    app.run()",
  "",
].join("\n");

// The second argument is Playwright's `use`; named `provide` because eslint's react-hooks rule
// treats any call to a function literally called `use` as a React hook.
const test = base.extend({
  context: async ({ browser }, provide) => {
    const context = await browser.newContext({ storageState: STATE, ignoreHTTPSErrors: true });
    await provide(context);
    await context.close();
  },
  page: async ({ context }, provide) => {
    await provide(await context.newPage());
  },
});

/** A fresh local user, added to the workspace as `ws_viewer` only, logged in through the real
 * `/api/auth/login` endpoint in its OWN browser context — never the admin's. */
async function newViewer(
  browser: Browser,
  adminPage: Page,
  wsSlug: string,
  h: Record<string, string>,
  tag: string,
): Promise<{ context: BrowserContext; page: Page }> {
  const email = `e2e-run-${tag}-${RUN_ID}@egret.local`;
  const password = "Viewer12345!";
  await postJson(adminPage.request, "/api/control-plane/users", h, { email, name: `E2E run ${tag}`, password });
  await addWorkspaceMember(adminPage.request, h, wsSlug, email, "ws_viewer");
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  const page = await context.newPage();
  const res = await page.request.post("/api/auth/login", { data: { email, password } });
  expect(res.ok(), `login ${email} \u2192 ${res.status()} ${await res.text()}`).toBeTruthy();
  return { context, page };
}

/** Opens the app through the Apps list's own Open link — the real path a viewer takes. */
async function openViaAppsList(viewerPage: Page, wsSlug: string, appName: string, appId: string): Promise<void> {
  await viewerPage.goto(`/explore/${wsSlug}/apps`);
  await viewerPage
    .getByRole("row", { name: new RegExp(appName) })
    .getByRole("link", { name: /Open/ })
    .click();
  await viewerPage.waitForURL(new RegExp(`/apps/${appId}$`));
  await expect(viewerPage.locator("iframe")).toBeAttached({ timeout: 120_000 });
}

/** The rendered `host=<pod name>` text: a real cross-pod fingerprint (see the module doc for why
 * this is `socket.gethostname()` and not `os.getpid()`). */
async function readHost(viewerPage: Page): Promise<string> {
  const el = viewerPage.frameLocator("iframe").locator("text=/host=\\S+/").first();
  await expect(el).toBeVisible({ timeout: 120_000 });
  const m = (await el.innerText()).match(/host=(\S+)/);
  if (!m) throw new Error("no host in the rendered app text");
  return m[1];
}

test.describe("data apps — run mode on kind (D24)", () => {
  test.skip(!ENABLED, "opt-in: EGRET_E2E_APPS=1 against the apps-enabled in-cluster kind deploy");
  test.setTimeout(300_000);

  test.beforeAll(async ({ browser }) => {
    const context = await browser.newContext({ ignoreHTTPSErrors: true });
    await loginLocalAdmin(await context.newPage());
    await context.storageState({ path: STATE });
    await context.close();
  });

  test("two viewers get two pods; an abandoned one is reaped; a third viewer does not disturb an active one", async ({
    page,
    browser,
  }) => {
    const ws = await firstWorkspace(page.request);
    const h = await csrf(page.context());

    // an admin publishes an app
    const created = await postJson(page.request, `/api/ws/${ws.slug}/apps`, h, { name: `e2e-run-${RUN_ID}` });
    const draft = await (await page.request.get(`/api/ws/${ws.slug}/apps/${created.id}/draft`)).json();
    const saved = await page.request.put(`/api/ws/${ws.slug}/apps/${created.id}/draft`, {
      headers: { ...h, "If-Match": `"${draft.revision}"` },
      data: { source: IDENTITY_APP },
    });
    expect(saved.ok(), await saved.text()).toBeTruthy();
    const published = await page.request.post(`/api/ws/${ws.slug}/apps/${created.id}/publish`, { headers: h });
    expect(published.ok(), await published.text()).toBeTruthy();

    const before = runPodNames();

    // two different viewers open the SAME published app
    const alice = await newViewer(browser, page, ws.slug, h, "alice");
    await openViaAppsList(alice.page, ws.slug, created.name, created.id);
    const aliceHost = await readHost(alice.page);

    const bob = await newViewer(browser, page, ws.slug, h, "bob");
    await openViaAppsList(bob.page, ws.slug, created.name, created.id);
    const bobHost = await readHost(bob.page);

    // 1. never the same pod: D24's one pod per (viewer, published version)
    expect(aliceHost, `alice and bob landed in the same pod: ${aliceHost}`).not.toBe(bobHost);
    const afterTwo = runPodNames();
    const newPods = afterTwo.filter((n) => !before.includes(n));
    expect(newPods.length, `expected 2 new run pods, saw ${JSON.stringify(newPods)}`).toBe(2);

    // 2. an abandoned viewer's pod is reaped; an actively-used one survives. Bob is kept "in use"
    // by reloading through the loop (a real session does this via its own traffic); alice is
    // simply closed and never touched again — a sleep-and-hope wait would not tell these apart.
    await alice.context.close();
    await expect
      .poll(
        async () => {
          await bob.page.reload();
          await expect(bob.page.locator("iframe")).toBeAttached({ timeout: 60_000 });
          return runPodNames().length;
        },
        { timeout: 120_000, intervals: [5_000] },
      )
      .toBe(afterTwo.length - 1);
    const bobHostAfterReap = await readHost(bob.page);
    expect(bobHostAfterReap, "bob's own pod must survive the reap").toBe(bobHost);

    // 3. a third viewer of the SAME app gets their own pod too, and bob's is untouched by it
    const carol = await newViewer(browser, page, ws.slug, h, "carol");
    await openViaAppsList(carol.page, ws.slug, created.name, created.id);
    const carolHost = await readHost(carol.page);
    expect(carolHost).not.toBe(bobHost);
    const afterCarol = runPodNames();
    expect(afterCarol.length, "bob's pod plus a new one for carol").toBe(afterTwo.length - 1 + 1);

    const bobHostFinal = await readHost(bob.page);
    expect(bobHostFinal, "a third viewer opening the app must not disturb an existing viewer's pod").toBe(bobHost);

    await bob.context.close();
    await carol.context.close();
  });
});
