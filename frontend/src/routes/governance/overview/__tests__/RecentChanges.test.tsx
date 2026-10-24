import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { focusManager } from "@tanstack/react-query";
import type { AttentionResponse, ChangeEntry, ChangesPage } from "@/api/generated/model";
import { timeAgo } from "@/lib/relativeTime";
import { openOverview, serve } from "@/routes/governance/overview/__tests__/harness";

afterEach(() => vi.unstubAllGlobals());

const ATTENTION: AttentionResponse = {
  generatedAt: "2026-10-18T10:00:00Z",
  counts: { catalogs: 0, grants: 0, rules: 0, tags: 0 },
  findings: [],
  notChecked: [],
};
const attentionOk = () => ({ status: 200, body: ATTENTION });

const entry = (id: number, over: Partial<ChangeEntry> = {}): ChangeEntry => ({
  id,
  at: `2026-10-${String(10 + (id % 9)).padStart(2, "0")}T08:00:00Z`,
  actor: { userId: "u-ana", label: "Ana" },
  impersonator: null,
  action: "perm_grant:create",
  sentence: `Ana created a grant #${id}`,
  target: { type: "grant", id: `g-${id}` },
  targetExists: true,
  detail: {},
  ...over,
});

const page = (entries: ChangeEntry[], nextCursor: string | null = null, retentionDays: number | null = 90): ChangesPage => ({
  entries,
  nextCursor,
  retentionDays,
});

const table = () => screen.findByTestId("recent-changes-table");

describe("Recent changes", () => {
  it("shows when, who, what and a link to a target that still exists", async () => {
    const e = entry(1, { at: "2026-10-12T08:00:00Z" });
    serve(attentionOk, () => ({ status: 200, body: page([e]) }));
    openOverview();
    const row = within(await table()).getByText("Ana created a grant #1").closest("tr")!;
    const time = row.querySelector("time")!;
    expect(time).toHaveAttribute("datetime", "2026-10-12T08:00:00Z");
    expect(time).toHaveTextContent(timeAgo("2026-10-12T08:00:00Z"));
    expect(within(row).getByText("Ana")).toBeInTheDocument();
    expect(within(row).getByRole("link", { name: "View" })).toHaveAttribute("href", "/governance/grants/g-1");
  });

  it("names an impersonated change \"Admin as Ana\" and a deleted actor \"A deleted user\"", async () => {
    serve(attentionOk, () => ({
      status: 200,
      body: page([
        entry(2, { impersonator: { userId: "u-admin", label: "Admin" } }),
        entry(3, { actor: { userId: "u-gone", label: null }, sentence: "A deleted user created a grant" }),
      ]),
    }));
    openOverview();
    const t = await table();
    expect(within(t).getByText("Admin as Ana")).toBeInTheDocument();
    expect(within(t).getByText("A deleted user", { selector: "td" })).toBeInTheDocument();
  });

  it("marks a deleted target and links nowhere", async () => {
    serve(attentionOk, () => ({ status: 200, body: page([entry(4, { targetExists: false })]) }));
    openOverview();
    const row = within(await table()).getByText("Ana created a grant #4").closest("tr")!;
    expect(within(row).getByText("(deleted)")).toBeInTheDocument();
    expect(within(row).queryByRole("link")).not.toBeInTheDocument();
  });

  it("fetches older changes with the cursor and appends them without duplicates", async () => {
    const first = Array.from({ length: 25 }, (_, i) => entry(100 - i));
    const { changesCalls } = serve(attentionOk, (q) =>
      q.get("cursor") === "c1"
        ? // The boundary row comes back again: it must not be shown twice.
          { status: 200, body: page([entry(76), entry(75), entry(74)], null) }
        : { status: 200, body: page(first, "c1") },
    );
    openOverview();
    await table();
    expect(screen.getAllByText(/^Ana created a grant #/)).toHaveLength(25);
    await userEvent.click(screen.getByRole("button", { name: "Show older" }));
    await waitFor(() => expect(screen.getAllByText(/^Ana created a grant #/)).toHaveLength(27));
    expect(changesCalls.at(-1)?.get("cursor")).toBe("c1");
    expect(changesCalls.at(-1)?.get("limit")).toBe("25");
    expect(screen.getAllByText("Ana created a grant #76")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Show older" })).not.toBeInTheDocument();
  });

  it("names no oldest date while older changes remain to load", async () => {
    serve(attentionOk, () => ({ status: 200, body: page([entry(11)], "c9") }));
    openOverview();
    await table();
    expect(screen.queryByText(/oldest change/)).not.toBeInTheDocument();
  });

  it("gives every link the shared focus ring (live check D17)", async () => {
    serve(attentionOk, () => ({ status: 200, body: page([entry(12)]) }));
    openOverview();
    const link = within(await table()).getByRole("link", { name: "View" });
    expect(link.className).toContain("focus-visible:ring-2");
    expect(link.className).toContain("focus-visible:ring-ring");
  });

  it("keeps the loaded changes and says why when older ones fail to load (review M7)", async () => {
    serve(attentionOk, (q) =>
      q.get("cursor") === "c1"
        ? { status: 500, body: { error: "older changes failed" } }
        : { status: 200, body: page([entry(13)], "c1") },
    );
    openOverview();
    await table();
    await userEvent.click(screen.getByRole("button", { name: "Show older" }));
    expect(await screen.findByText("older changes failed")).toBeInTheDocument();
    expect(screen.getByText("Ana created a grant #13")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show older" })).toBeEnabled();
  });

  it("drops appended pages when the first page is fetched again (review M8)", async () => {
    let first = page(Array.from({ length: 2 }, (_, i) => entry(100 - i)), "c1");
    serve(attentionOk, (q) =>
      q.get("cursor") === "c1"
        ? { status: 200, body: page([entry(50)], null) }
        : { status: 200, body: first },
    );
    openOverview();
    await table();
    await userEvent.click(screen.getByRole("button", { name: "Show older" }));
    expect(await screen.findByText("Ana created a grant #50")).toBeInTheDocument();
    // A new change lands; the first page is fetched again (window focus).
    first = page([entry(101), entry(100), entry(99)], "c2");
    focusManager.setFocused(false);
    focusManager.setFocused(true);
    expect(await screen.findByText("Ana created a grant #101")).toBeInTheDocument();
    // The appended page belonged to the old first page: it is gone, and Show
    // older follows the new page's cursor.
    expect(screen.queryByText("Ana created a grant #50")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show older" })).toBeInTheDocument();
  });

  it("offers no Show older when there is no cursor", async () => {
    serve(attentionOk, () => ({ status: 200, body: page([entry(5)], null) }));
    openOverview();
    await table();
    expect(screen.queryByRole("button", { name: "Show older" })).not.toBeInTheDocument();
  });

  it("says a classification was attached or removed, with no trailing dot on a table (N18, finding 6)", async () => {
    serve(attentionOk, () => ({
      status: 200,
      body: page([
        // The server's sentence for a table-level assignment: the column is "".
        entry(20, {
          action: "tag:assign",
          sentence: "Ana tagged demo_warehouse.analytics.orders. with uxwalk-tbl",
          detail: { tag_id: "t-1" },
        }),
        entry(21, {
          action: "tag:unassign",
          sentence: "Admin as Ana removed uxwalk-pii from demo_warehouse.analytics.customers.email",
          impersonator: { userId: "u-admin", label: "Admin" },
          detail: { tag_id: "t-2" },
        }),
        entry(22, {
          action: "tag:unassign",
          sentence: "Ana removed a deleted classification from demo_warehouse.analytics.orders.",
          detail: { tag_id: "t-3" },
        }),
      ]),
    }));
    openOverview();
    const t = await table();
    expect(within(t).getByText("Ana attached classification uxwalk-tbl to demo_warehouse.analytics.orders")).toBeInTheDocument();
    expect(
      within(t).getByText("Admin as Ana removed classification uxwalk-pii from demo_warehouse.analytics.customers.email"),
    ).toBeInTheDocument();
    expect(within(t).getByText("Ana removed a deleted classification from demo_warehouse.analytics.orders")).toBeInTheDocument();
    expect(t).not.toHaveTextContent(/tagged/);
  });

  it("says what a rule does with the glossary's mask label and a classification or path, never MASK_* or tag: (N18)", async () => {
    serve(attentionOk, () => ({
      status: 200,
      body: page([
        entry(30, {
          action: "perm_rule:create",
          sentence: "Ana created rule Mask uxwalk-pii: mask MASK on tag:uxwalk-pii",
          target: { type: "rule", id: "r-1" },
          detail: {
            before: null,
            after: { name: "Mask uxwalk-pii", kind: "mask", selector: "tag:uxwalk-pii", function: "MASK", exempt: [], exempt_exclude: [] },
          },
        }),
        entry(31, {
          action: "perm_rule:delete",
          sentence: "Ana deleted rule EU only: row filter region = 'EU' on ice.sales.orders",
          target: { type: "rule", id: "r-2" },
          detail: {
            before: { name: "EU only", kind: "row_filter", selector: "ice.sales.orders", function: "region = 'EU'", exempt: [], exempt_exclude: [] },
            after: null,
          },
        }),
        entry(32, {
          action: "perm_rule:update",
          sentence: "Ana changed rule Hash it: mask MASK_HASH on ice.sales.orders.email",
          target: { type: "rule", id: "r-3" },
          detail: {
            before: { name: "Hash it", kind: "mask", selector: "ice.sales.orders.email", function: "MASK", exempt: [], exempt_exclude: [] },
            after: { name: "Hash it", kind: "mask", selector: "ice.sales.orders.email", function: "MASK_HASH", exempt: [], exempt_exclude: [] },
          },
        }),
      ]),
    }));
    openOverview();
    const t = await table();
    expect(within(t).getByText("Ana created rule Mask uxwalk-pii: Redact mask on classification uxwalk-pii")).toBeInTheDocument();
    expect(within(t).getByText("Ana deleted rule EU only: row filter region = 'EU' on path ice.sales.orders")).toBeInTheDocument();
    expect(within(t).getByText("Ana changed rule Hash it: Hash mask on path ice.sales.orders.email")).toBeInTheDocument();
    expect(t).not.toHaveTextContent(/MASK|tag:/);
  });

  // Username takeover (spec 2026-10-23 §4.4, D4): the server leaves the row out
  // for a reader without `user:read`, so the client renders what it is sent.
  it("says who deleted which user, with no link to the deleted user", async () => {
    serve(attentionOk, () => ({
      status: 200,
      body: page([
        entry(41, {
          action: "user:delete",
          sentence: "Ana deleted user bob",
          target: null,
          targetExists: false,
          detail: { before: { username: "bob", email: "bob@example.com" }, after: null },
        }),
      ]),
    }));
    openOverview();
    const row = within(await table()).getByText("Ana deleted user bob").closest("tr")!;
    expect(within(row).queryByRole("link")).not.toBeInTheDocument();
    expect(within(row).queryByText("(deleted)")).not.toBeInTheDocument();
  });

  it("keeps the server's sentence for a change too large to record its details", async () => {
    const sentence = "Ana created a rule; the change was too large to record its details.";
    serve(attentionOk, () => ({
      status: 200,
      body: page([entry(40, { action: "perm_rule:create", sentence, target: { type: "rule", id: "r-4" }, detail: { truncated: true, size: 20000 } })]),
    }));
    openOverview();
    expect(within(await table()).getByText(sentence)).toBeInTheDocument();
  });

  it("says how long changes are kept, and that role and classification events are best effort", async () => {
    serve(attentionOk, () => ({ status: 200, body: page([entry(6)], null, 90) }));
    openOverview();
    expect(await screen.findByText(/Changes older than 90 days are pruned\./)).toBeInTheDocument();
    // Live check D13: a reader sees only their families, so the oldest entry
    // they see is not when recording began; the footnote says what it knows.
    expect(screen.getByText(/^The oldest change you can see is from /)).toBeInTheDocument();
    expect(screen.queryByText(/Recorded since/)).not.toBeInTheDocument();
    expect(
      screen.getByText(
        "Role assignments and classifications attached to data are recorded after they commit and can be missing if the audit writer was overloaded.",
      ),
    ).toBeInTheDocument();
  });

  it("says changes are never pruned when no retention applies", async () => {
    serve(attentionOk, () => ({ status: 200, body: page([entry(7)], null, null) }));
    openOverview();
    expect(await screen.findByText(/Changes are never pruned\./)).toBeInTheDocument();
  });

  it("explains an empty feed", async () => {
    serve(attentionOk, () => ({ status: 200, body: page([], null) }));
    openOverview();
    expect(await screen.findByText("No governance change has been recorded yet.")).toBeInTheDocument();
    expect(
      screen.getByText("Recording began when this version was installed; earlier changes are not listed."),
    ).toBeInTheDocument();
  });

  it("shows the server's message and retries", async () => {
    let calls = 0;
    serve(attentionOk, () => {
      calls += 1;
      return calls === 1 ? { status: 500, body: { error: "the feed failed" } } : { status: 200, body: page([entry(8)]) };
    });
    openOverview();
    expect(await screen.findByText("the feed failed")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Ana created a grant #8")).toBeInTheDocument();
  });

  it("shows a skeleton while loading", async () => {
    serve(attentionOk, () => "never");
    const { container } = openOverview();
    await screen.findByText("Nothing needs attention.");
    expect(container.querySelector('[data-slot="skeleton"], .animate-pulse')).not.toBeNull();
    expect(screen.queryByTestId("recent-changes-table")).not.toBeInTheDocument();
  });

  it("still renders when the attention request fails, and the other way round", async () => {
    serve(() => ({ status: 500, body: { error: "attention failed" } }), () => ({ status: 200, body: page([entry(9)]) }));
    openOverview();
    expect(await screen.findByText("attention failed")).toBeInTheDocument();
    expect(await screen.findByText("Ana created a grant #9")).toBeInTheDocument();
  });

  it("keeps Needs attention when the feed fails", async () => {
    serve(attentionOk, () => ({ status: 500, body: { error: "the feed failed" } }));
    openOverview();
    expect(await screen.findByText("the feed failed")).toBeInTheDocument();
    expect(screen.getByText("Nothing needs attention.")).toBeInTheDocument();
  });

  it("renders cards on a narrow viewport", async () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn((q: string) => ({
        matches: true,
        media: q,
        addEventListener: () => {},
        removeEventListener: () => {},
      })),
    );
    serve(attentionOk, () => ({ status: 200, body: page([entry(10)]) }));
    openOverview();
    const cards = await screen.findByTestId("recent-changes-cards");
    expect(within(cards).getByText("Ana created a grant #10")).toBeInTheDocument();
    expect(screen.queryByTestId("recent-changes-table")).not.toBeInTheDocument();
  });
});

describe("Recent changes as a link target", () => {
  it("is the section #recent-changes, which the Migrate tab links to", async () => {
    serve(attentionOk, () => ({ status: 200, body: page([entry(1)]) }));
    openOverview();
    await table();
    const target = document.getElementById("recent-changes");
    expect(target?.tagName).toBe("SECTION");
    expect(within(target as HTMLElement).getByRole("heading", { name: "Recent changes" })).toBeInTheDocument();
    expect(within(target as HTMLElement).getByTestId("recent-changes-table")).toBeInTheDocument();
  });
});

