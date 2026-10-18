import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

  it("offers no Show older when there is no cursor", async () => {
    serve(attentionOk, () => ({ status: 200, body: page([entry(5)], null) }));
    openOverview();
    await table();
    expect(screen.queryByRole("button", { name: "Show older" })).not.toBeInTheDocument();
  });

  it("says how long changes are kept, and that role and tag events are best effort", async () => {
    serve(attentionOk, () => ({ status: 200, body: page([entry(6)], null, 90) }));
    openOverview();
    expect(await screen.findByText(/Changes older than 90 days are pruned\./)).toBeInTheDocument();
    // Live check D13: a reader sees only their families, so the oldest entry
    // they see is not when recording began; the footnote says what it knows.
    expect(screen.getByText(/^The oldest change you can see is from /)).toBeInTheDocument();
    expect(screen.queryByText(/Recorded since/)).not.toBeInTheDocument();
    expect(
      screen.getByText(
        "Role assignments and catalog tags are recorded after they commit and can be missing if the audit writer was overloaded.",
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
