import { test, expect, type Page, type Locator } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace } from "./helpers";

/**
 * Per-workspace feature entitlements against the ordinary dev stack (docker
 * compose + `just serve` + `npm run dev` — see README.md), NOT the in-cluster
 * kind deploy: this is pure browser UI with no k8s provisioning, so it targets
 * the plain local stack like `embedded-catalog.spec.ts`. Opt-in via
 * `EGRET_E2E_FEATURES=1`, matching the `EGRET_E2E_CATALOG` gating convention
 * for a live-stack-but-not-kind spec.
 *
 * Proves a platform admin turning a module OFF for a workspace (Control Plane
 * → Workspaces → detail → Features tab) actually enforces, not just decorates
 * the UI:
 *   1. the Explore nav no longer offers the disabled module (still offers one
 *      that stayed on);
 *   2. navigating straight to its URL renders Forbidden, not the module;
 *   3. its API 403s with the `feature_disabled` machine-readable code.
 *
 * Login is rate-limited (5 attempts / 5 min per ip+email) — this spec signs in
 * ONCE (`loginLocalAdmin`, break-glass platform admin — platform admins see
 * every workspace via `/api/me/workspaces` and, per the feature's own design,
 * get no bypass on entitlements either, so the same session drives both the
 * Control Plane toggle and the Explore-side checks).
 *
 * The suite has no disposable-workspace helper (only `firstWorkspace`, the
 * shared seeded workspace other specs also read), so this mutates that shared
 * workspace's Apps entitlement and restores it in a `finally` — matching
 * the cleanup idiom used elsewhere in this suite (e.g. `dataplane-kind.spec.ts`
 * deleting its engine in `finally`) — so a failure mid-test can't leave
 * Apps off and poison a later run. The `try` starts right before the
 * switch is flipped off (the only step that needs undoing), not after all the
 * assertions, so any assertion failure below still runs the restore.
 *
 * NOTE: this does NOT assert on the "Workspace features updated" toast
 * (`WorkspaceFeaturesTab.tsx`'s real success text). Confirmed live: the app
 * never mounts a `<Toaster />`/`sonner` host anywhere in `App.tsx` — every
 * `toast.success()`/`toast.error()` call in the codebase is a no-op on
 * screen (verified both by grepping `src` for `<Toaster` and by watching a
 * real save through the browser: the mutation persists correctly but no
 * toast ever appears). That's a pre-existing, app-wide gap outside this
 * spec's scope (`frontend/tests/e2e/` only) to fix. Save's own
 * enabled/disabled state — wired to `dirty`, i.e. the draft matching what
 * was just persisted — is used instead as the observable "save succeeded"
 * signal.
 */
const ENABLED = process.env.EGRET_E2E_FEATURES === "1";

/** Click Save and wait for it to go back to disabled (draft caught up with
 * the just-persisted features) — the real, observable "save succeeded"
 * signal, since no toast ever renders (see file-level note above). */
async function saveFeatures(page: Page): Promise<void> {
  const save = page.getByRole("button", { name: "Save", exact: true });
  await save.click();
  await expect(save).toBeDisabled();
}

test.describe("workspace feature entitlements", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_FEATURES=1 with the live dev stack up");
  test.setTimeout(90_000);

  test("disabling Apps hides the nav item, blocks the route and the API, and leaves Charts reachable", async ({
    page,
  }) => {
    await loginLocalAdmin(page);
    const { id: wsId, slug } = await firstWorkspace(page.request);

    // Baseline: Apps is on before we touch anything (the seeded workspace
    // starts with every module enabled — an absent row means enabled).
    await page.goto(`/explore/${slug}/apps`);
    await expect(page.getByText("403 — Forbidden")).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "Apps" })).toBeVisible();
    await expect(page.locator("nav").getByRole("link", { name: "Apps" })).toBeVisible();

    // Control Plane → Workspaces → detail → Features tab.
    const gotoFeaturesTab = () => page.goto(`/control-plane/workspaces/${wsId}?tab=features`);
    const appsSwitch: Locator = page.getByRole("switch", { name: "Apps" });
    await gotoFeaturesTab();
    await expect(appsSwitch).toBeVisible();
    await expect(appsSwitch).toBeChecked();

    try {
      // Turn Apps off and save.
      await appsSwitch.click();
      await expect(appsSwitch).not.toBeChecked();
      await saveFeatures(page);

      // 1. Explore nav: Apps is gone, Charts (left on) is still there.
      await page.goto(`/explore/${slug}/charts`);
      await expect(page.locator("nav").getByRole("link", { name: "Apps" })).toHaveCount(0);
      await expect(page.locator("nav").getByRole("link", { name: "Charts" })).toBeVisible();

      // 2. Direct navigation to the apps URL is blocked — Forbidden renders,
      // the apps page does not.
      await page.goto(`/explore/${slug}/apps`);
      await expect(page.getByText("403 — Forbidden")).toBeVisible();
      await expect(page.getByText("You don't have access to this page.")).toBeVisible();
      await expect(page.getByRole("heading", { name: "Apps" })).toHaveCount(0);

      // 3. A module that stayed on is still reachable (not Forbidden).
      await page.goto(`/explore/${slug}/charts`);
      await expect(page.getByText("403 — Forbidden")).toHaveCount(0);
      await expect(page.getByRole("heading", { name: "Charts" })).toBeVisible();

      // The API enforces the same thing server-side (WsContext), independent of
      // this client-side nav/route gating: 403 with the machine-readable code.
      const appsApi = await page.request.get(`/api/ws/${slug}/apps`);
      expect(appsApi.status(), await appsApi.text()).toBe(403);
      expect((await appsApi.json()).code).toBe("feature_disabled");

      // Charts' own API is unaffected.
      const chartsApi = await page.request.get(`/api/ws/${slug}/charts`);
      expect(chartsApi.ok(), await chartsApi.text()).toBeTruthy();
    } finally {
      // Restore Apps so the shared workspace isn't left poisoned for
      // later runs (this spec's own baseline check included).
      await gotoFeaturesTab();
      if (!(await appsSwitch.isChecked())) {
        await appsSwitch.click();
        await expect(appsSwitch).toBeChecked();
        await saveFeatures(page);
      }
    }

    // Confirm the restore actually took: Apps is reachable again.
    await page.goto(`/explore/${slug}/apps`);
    await expect(page.getByText("403 — Forbidden")).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "Apps" })).toBeVisible();
    await expect(page.locator("nav").getByRole("link", { name: "Apps" })).toBeVisible();
  });
});
