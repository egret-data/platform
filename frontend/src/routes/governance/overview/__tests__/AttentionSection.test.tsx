import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { AttentionResponse, Finding } from "@/api/generated/model";
import { openOverview, serve } from "@/routes/governance/overview/__tests__/harness";

afterEach(() => vi.unstubAllGlobals());

const COUNTS = { catalogs: 3, grants: 1, rules: 2, tags: 1 };

function response(over: Partial<AttentionResponse> = {}): AttentionResponse {
  return { generatedAt: "2026-10-18T10:00:00Z", counts: COUNTS, findings: [], notChecked: [], ...over };
}

const finding = (over: Partial<Finding> & Pick<Finding, "kind" | "severity">): Finding => ({
  count: 1,
  sentence: `${over.kind} sentence`,
  sample: [],
  ref: null,
  source: null,
  ...over,
});

const ok = (body: AttentionResponse) => () => ({ status: 200, body });

describe("the counts strip", () => {
  it("shows the four counts the reader may read, with singulars", async () => {
    serve(ok(response()));
    openOverview();
    const strip = await screen.findByTestId("overview-counts");
    expect(strip).toHaveTextContent("3catalogs");
    expect(strip).toHaveTextContent("1grant");
    expect(strip).not.toHaveTextContent("egret grant");
    expect(strip).toHaveTextContent("2rules");
    expect(strip).toHaveTextContent("1classification");
    expect(strip).not.toHaveTextContent("classifications");
  });

  it("leaves out a count the reader may not read, rather than showing 0", async () => {
    serve(ok(response({ counts: { catalogs: 0, grants: 4, rules: 0, tags: null } })));
    openOverview();
    const strip = await screen.findByTestId("overview-counts");
    expect(strip).toHaveTextContent("0catalogs");
    expect(strip).toHaveTextContent("4grants");
    expect(strip).not.toHaveTextContent("egret grant");
    expect(strip).not.toHaveTextContent("classification");
  });
});

describe("Needs attention", () => {
  it("groups findings under Critical, Warning and Info, in that order, and renders no empty group", async () => {
    serve(
      ok(
        response({
          findings: [
            // Out of order on purpose: the page groups, it does not trust the order.
            finding({ kind: "tags_without_clearance", severity: "info", sentence: "2 classifications have no clearance: they gate nothing." }),
            finding({ kind: "engine_failed", severity: "critical", sentence: "1 engine egret provisions failed." }),
            finding({ kind: "compile_failed", severity: "critical", sentence: "The last compile of 1 profile failed." }),
          ],
        }),
      ),
    );
    openOverview();
    const headings = await screen.findAllByRole("heading", { level: 3 });
    expect(headings.map((h) => h.textContent)).toEqual(["Critical", "Info"]);
    const critical = headings[0].closest("section")!;
    expect(within(critical).getByText("1 engine egret provisions failed.")).toBeInTheDocument();
    // The severity is a word, not only a colour.
    expect(within(critical).getAllByText("Critical")).toHaveLength(3);
  });

  it("shows the sentence, linked samples, a text sample without a ref, and Open", async () => {
    serve(
      ok(
        response({
          findings: [
            finding({
              kind: "plugin_quiet",
              severity: "warning",
              count: 2,
              sentence: "No plugin has polled 2 profiles for 5 minutes or more.",
              ref: { type: "profiles" },
              sample: [
                { label: "trino-prod", detail: "No plugin has polled profile trino-prod for 9 minutes.", ref: { type: "engine", id: "e1", name: "t1" } },
                { label: "sr-prod", detail: null, ref: null },
              ],
            }),
          ],
        }),
      ),
    );
    openOverview();
    const row = (await screen.findByText("No plugin has polled 2 profiles for 5 minutes or more.")).closest("li")!;
    expect(within(row).getByText("Warning")).toBeInTheDocument();
    expect(within(row).getByRole("link", { name: "trino-prod" })).toHaveAttribute("href", "/control-plane/engines/e1");
    expect(within(row).getByText(/No plugin has polled profile trino-prod for 9 minutes\./)).toBeInTheDocument();
    expect(within(row).queryByRole("link", { name: "sr-prod" })).not.toBeInTheDocument();
    expect(within(row).getByText("sr-prod")).toBeInTheDocument();
    expect(within(row).getByRole("link", { name: "Open" })).toHaveAttribute("href", "/governance/engines");
    // Live check D17: the shared focus ring on every link of the row.
    for (const a of within(row).getAllByRole("link")) {
      expect(a.className).toContain("focus-visible:ring-2");
      expect(a.className).toContain("focus-visible:ring-ring");
    }
  });

  // #4a spec §3: a profile link opens the engine page's tab where the finding is fixed.
  it.each([
    ["plugin_silent", "plugin"],
    ["plugin_quiet", "plugin"],
    ["plugin_behind", "plugin"],
    ["policy_warnings", "policies"],
    ["rule_overlaps", "overlaps"],
  ])("links a %s profile sample and Open to ?tab=%s", async (kind, tab) => {
    serve(
      ok(
        response({
          findings: [
            finding({
              kind,
              severity: "warning",
              sentence: `${kind} about p/1`,
              ref: { type: "profile", name: "p/1" },
              sample: [{ label: "p/1", detail: null, ref: { type: "profile", name: "p/1" } }],
            }),
          ],
        }),
      ),
    );
    openOverview();
    const row = (await screen.findByText(`${kind} about p/1`)).closest("li")!;
    const want = `/governance/engines/profiles/p%2F1?tab=${tab}`;
    expect(within(row).getByRole("link", { name: "Open" })).toHaveAttribute("href", want);
    expect(within(row).getByRole("link", { name: "p/1" })).toHaveAttribute("href", want);
  });

  it("puts no tab on a profile link of a finding kind fixed elsewhere", async () => {
    serve(
      ok(
        response({
          findings: [
            finding({ kind: "compile_failed", severity: "critical", sentence: "compile about p", ref: { type: "profile", name: "p" } }),
          ],
        }),
      ),
    );
    openOverview();
    const row = (await screen.findByText("compile about p")).closest("li")!;
    expect(within(row).getByRole("link", { name: "Open" })).toHaveAttribute("href", "/governance/engines/profiles/p");
  });

  it("shows at most five samples even if the server sends more (review M7)", async () => {
    const sample = Array.from({ length: 7 }, (_, i) => ({ label: `s${i}`, detail: null, ref: null }));
    serve(ok(response({ findings: [finding({ kind: "engine_failed", severity: "critical", count: 7, sample })] })));
    openOverview();
    const row = (await screen.findByText("engine_failed sentence")).closest("li")!;
    expect(within(row).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["s0", "s1", "s2", "s3", "s4"]);
  });

  it("renders a source it could not check as a warning with no link", async () => {
    serve(
      ok(
        response({
          findings: [
            finding({ kind: "source_unavailable", severity: "warning", source: "rule_overlaps", sentence: "Could not check rule overlaps." }),
          ],
        }),
      ),
    );
    openOverview();
    const row = (await screen.findByText("Could not check rule overlaps.")).closest("li")!;
    expect(within(row).getByText("Warning")).toBeInTheDocument();
    expect(within(row).queryByRole("link")).not.toBeInTheDocument();
    // A failed check is not a clean bill.
    expect(screen.queryByText(/Nothing needs attention/)).not.toBeInTheDocument();
  });

  it("says nothing needs attention only when every check ran and found nothing", async () => {
    serve(ok(response()));
    openOverview();
    expect(await screen.findByText("Nothing needs attention.")).toBeInTheDocument();
  });

  it("qualifies the empty state when some checks were not run, and names their permissions", async () => {
    serve(
      ok(
        response({
          notChecked: [
            { kind: "plugin_silent", requires: ["policy:read", "engine:read"], any: false },
            { kind: "plugin_behind", requires: ["policy:read", "engine:read"], any: false },
            { kind: "tags_without_clearance", requires: ["tag:read"], any: false },
          ],
        }),
      ),
    );
    openOverview();
    expect(await screen.findByText("Nothing needs attention among the checks you can run.")).toBeInTheDocument();
    expect(screen.queryByText("Nothing needs attention.")).not.toBeInTheDocument();
    expect(
      screen.getByText("Not checked for you: engine liveness and plugin drift (needs `policy:read` and `engine:read`)."),
    ).toBeInTheDocument();
    expect(screen.getAllByText(/^Not checked for you: engine liveness/)).toHaveLength(1);
    expect(screen.getByText("Not checked for you: classification clearances (needs `tag:read`).")).toBeInTheDocument();
  });

  // Spec 2026-10-20-ranger-surface-auth A12: an engine whose Ranger credential is rejected.
  it("names the engine credential check and links a rejected engine to its page", async () => {
    serve(
      ok(
        response({
          findings: [
            finding({
              kind: "engine_auth_failing",
              severity: "critical",
              count: 2,
              sentence: "The Ranger credential of 2 engines is being rejected.",
              ref: { type: "engines" },
              sample: [
                {
                  label: "trino-prod",
                  detail: "42 rejected credentials, the last at 2026-10-20 10:00:00 UTC; no successful download since.",
                  ref: { type: "engine", id: "e1", name: "trino-prod" },
                },
              ],
            }),
          ],
          notChecked: [{ kind: "engine_auth_failing", requires: ["engine:read"], any: false }],
        }),
      ),
    );
    openOverview();
    const row = (await screen.findByText("The Ranger credential of 2 engines is being rejected.")).closest("li")!;
    expect(within(row).getByText("Critical")).toBeInTheDocument();
    expect(within(row).getByRole("link", { name: "trino-prod" })).toHaveAttribute("href", "/control-plane/engines/e1");
    expect(within(row).getByText(/42 rejected credentials/)).toBeInTheDocument();
    expect(
      screen.getByText("Not checked for you: engine Ranger credentials (needs `engine:read`)."),
    ).toBeInTheDocument();
  });

  // Username takeover (spec 2026-10-23 §4.5): platform admins only.
  it("opens the Reused names tab from a username_reuse finding", async () => {
    serve(
      ok(
        response({
          findings: [
            finding({
              kind: "username_reuse",
              severity: "critical",
              count: 1,
              sentence: "1 engine username is written for someone other than its holder.",
              ref: { type: "reused_names" },
              sample: [{ label: "bob", detail: "grant, include", ref: { type: "grant", id: "g-bob" } }],
            }),
          ],
        }),
      ),
    );
    openOverview();
    const row = (await screen.findByText("1 engine username is written for someone other than its holder.")).closest("li")!;
    expect(within(row).getByRole("link", { name: "Open" })).toHaveAttribute("href", "/governance/people?kind=reused");
    expect(within(row).getByRole("link", { name: "bob" })).toHaveAttribute("href", "/governance/grants/g-bob");
  });

  // Classifications follow a rename (spec 2026-10-26 §5.8, C15, R10).
  it("names the stale classification checks for a reader without tag:read", async () => {
    serve(
      ok(
        response({
          notChecked: [
            { kind: "classifications_on_missing_tables", requires: ["tag:read"], any: false },
            { kind: "classifications_on_dropped_tables", requires: ["tag:read"], any: false },
            { kind: "classifications_on_missing_columns", requires: ["tag:read"], any: false },
            { kind: "renamed_names_reused", requires: ["tag:read"], any: false },
          ],
        }),
      ),
    );
    openOverview({ user_id: "p", email: "p@x", platform_roles: [], permissions: ["policy:read"] });
    for (const what of [
      "classifications on tables that do not exist",
      "classifications on dropped tables",
      "classifications on columns that do not exist",
      "table names a rename left unclassified under a new table",
    ]) {
      expect(await screen.findByText(`Not checked for you: ${what} (needs \`tag:read\`).`)).toBeInTheDocument();
    }
  });

  // Grants follow a rename (spec 2026-10-27 §5.8, D16, D17).
  it("names the stale permission checks for a reader who may not run them", async () => {
    serve(
      ok(
        response({
          notChecked: [
            { kind: "permissions_on_missing_tables", requires: ["policy:read"], any: false },
            { kind: "permissions_on_missing_columns", requires: ["policy:read"], any: false },
            { kind: "renamed_names_lost_grants", requires: ["policy:read"], any: false },
            { kind: "catalog_grants_on_missing_tables", requires: ["catalog_registry:read"], any: false },
            { kind: "renamed_names_lost_catalog_grants", requires: ["catalog_registry:read"], any: false },
          ],
        }),
      ),
    );
    openOverview({ user_id: "t", email: "t@x", platform_roles: [], permissions: ["tag:read"] });
    for (const [what, perm] of [
      ["permissions on tables that do not exist", "policy:read"],
      ["permissions on columns that do not exist", "policy:read"],
      ["table names a rename left without their allow grants", "policy:read"],
      ["catalog grants on tables that do not exist", "catalog_registry:read"],
      ["table names a rename left without their catalog grants", "catalog_registry:read"],
    ]) {
      expect(await screen.findByText(`Not checked for you: ${what} (needs \`${perm}\`).`)).toBeInTheDocument();
    }
  });

  it("names the reused-username check for a reader who is not a platform admin", async () => {
    serve(ok(response({ notChecked: [{ kind: "username_reuse", requires: ["platform_admin"], any: false }] })));
    openOverview({ user_id: "p", email: "p@x", platform_roles: [], permissions: ["policy:read"] });
    expect(
      await screen.findByText("Not checked for you: engine usernames reused after a delete (needs `platform_admin`)."),
    ).toBeInTheDocument();
  });

  it("links the fixed Not checked here line to Classifications only for a tag:read reader", async () => {
    serve(ok(response()));
    openOverview({ user_id: "p", email: "p@x", platform_roles: [], permissions: ["policy:read"] });
    const line = await screen.findByText(/^Not checked here: views that may expose classified data/);
    expect(within(line).queryByRole("link")).not.toBeInTheDocument();
  });

  it("links the fixed Not checked here line for a tag:read reader", async () => {
    serve(ok(response()));
    openOverview({ user_id: "t", email: "t@x", platform_roles: [], permissions: ["tag:read"] });
    const line = await screen.findByText(/^Not checked here: views that may expose classified data/);
    expect(within(line).getByRole("link", { name: "see Classifications" })).toHaveAttribute(
      "href",
      "/governance/classifications",
    );
  });

  it("shows a skeleton while loading", async () => {
    serve(() => "never");
    const { container } = openOverview();
    await waitFor(() => expect(container.querySelector('[data-slot="skeleton"], .animate-pulse')).not.toBeNull());
    expect(screen.queryByText(/Nothing needs attention/)).not.toBeInTheDocument();
  });

  it("shows the server's message and retries", async () => {
    let calls = 0;
    serve(() => {
      calls += 1;
      return calls === 1 ? { status: 500, body: { error: "the snapshot failed" } } : { status: 200, body: response() };
    });
    openOverview();
    expect(await screen.findByText("the snapshot failed")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Nothing needs attention.")).toBeInTheDocument();
  });
});
