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
    expect(strip).toHaveTextContent("1egret grant");
    expect(strip).toHaveTextContent("2egret rules");
    expect(strip).toHaveTextContent("1classification");
    expect(strip).not.toHaveTextContent("classifications");
  });

  it("leaves out a count the reader may not read, rather than showing 0", async () => {
    serve(ok(response({ counts: { catalogs: 0, grants: 4, rules: 0, tags: null } })));
    openOverview();
    const strip = await screen.findByTestId("overview-counts");
    expect(strip).toHaveTextContent("0catalogs");
    expect(strip).toHaveTextContent("4egret grants");
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

  it("shows the sentence, up to five linked samples, a text sample without a ref, and Open", async () => {
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
    expect(within(row).getByRole("link", { name: "Open" })).toHaveAttribute("href", "/governance/policies");
  });

  it("renders a source it could not check as a warning with no link", async () => {
    serve(
      ok(
        response({
          findings: [
            finding({ kind: "source_unavailable", severity: "warning", source: "rule_overlaps", sentence: "Could not check rule overlaps: timeout" }),
          ],
        }),
      ),
    );
    openOverview();
    const row = (await screen.findByText("Could not check rule overlaps: timeout")).closest("li")!;
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
