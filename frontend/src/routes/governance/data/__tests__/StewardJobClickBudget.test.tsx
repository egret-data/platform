import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import type { BrowseResponse, ObjectTags } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import DataRoutes from "@/routes/governance/data/DataRoutes";
import { setAttachIntent } from "@/routes/governance/attachIntent";
import { browseItems, catalog, serveData, type FakeData } from "./fakeDataServer";

/**
 * The steward job's click budget (classification UX live result §2; Task 13).
 * Job: mask `email` of `ice.sales.orders` for everyone except group
 * `data-team`, with a new classification.
 *
 * The live walkthrough counted 20 clicks. Its first 8 (Governance,
 * Classifications, New classification, Create classification, Attach in Data,
 * the Go-to-path box, Go, `email`) reach the column panel on pages this
 * package does not change; they are not re-driven here. This test drives the
 * rest, from the column panel to a created rule, through the real pages and
 * the real generated client, and counts every click. Typing is not a click
 * (the live count's rule).
 *
 * Budget: 7 clicks here, so 8 + 7 = 15 for the whole job (target ≤ 15).
 * Each click is named below; a change that adds one fails this test.
 */

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("sonner", () => ({ toast }));
afterEach(() => vi.unstubAllGlobals());

const ok = (body: unknown) => ({ status: 200, body });
const ICE = { ...catalog("c-ice", "ice", "iceberg", true), engines: [] };
const COLUMNS: BrowseResponse = {
  source: { kind: "egret" },
  engines: [],
  columns: [
    { name: "id", type: "long" },
    { name: "email", type: "string" },
  ],
};
const UXWALK = {
  id: "t-uxwalk",
  key: "uxwalk-pii",
  name: "uxwalk-pii",
  description: "",
  color: "#336699",
  assignment_count: 0,
  created_at: "2026-10-22T00:00:00Z",
  required_roles: [],
  required_groups: [],
};
const NO_TAGS: ObjectTags = { supported: true, assignments: [] };
const ON_EMAIL: ObjectTags = { supported: true, assignments: [{ schema: "sales", table: "orders", column: "email", tag: UXWALK }] };
const EFFECT = {
  kind: "mask",
  catalogs: [{ catalogId: "c-ice", catalogName: "ice", engines: [], notEnforced: "no_engine" }],
  tableCount: 1,
  columnCount: 1,
  enforcedBy: 0,
  notEnforced: "no_engine",
  exemptMembers: 1,
  exemptEveryone: false,
  alreadyMasked: [],
  truncated: false,
};
const STEWARD: Me = {
  user_id: "s1",
  email: "s@egret.local",
  platform_roles: [],
  permissions: ["policy:read", "policy:update", "tag:read", "catalog_registry:update"],
};

describe("the steward job's click budget", () => {
  it("goes from the column panel to a created rule in 7 clicks (15 for the whole job)", async () => {
    const data: FakeData = {
      catalogs: ok([ICE]),
      browse: {
        "c-ice": ok(browseItems([{ name: "sales", kind: "schema" }])),
        "c-ice|sales": ok(browseItems([{ name: "orders", kind: "table" }])),
        "c-ice|sales|orders": ok(COLUMNS),
      },
      tags: { "c-ice|sales|orders": ok(NO_TAGS) },
      rules: ok([]),
      grants: ok([]),
      tagCatalogue: ok([UXWALK]),
      principals: ok([{ kind: "group", name: "data-team", label: "data-team", username: null }]),
      preview: ok({ affected: [], compile: { services: [], created: 0, updated: 0, deleted: 0, unchanged: 0, skipped: [] }, effect: EFFECT }),
      ruleWrite: { status: 201, body: { id: "r-new", compile: { services: [], created: 0, updated: 0, deleted: 0, unchanged: 0, skipped: [] } } },
    };
    const { permCalls, tagWrites } = serveData(data);
    const user = userEvent.setup();
    const clicked: string[] = [];
    const click = async (what: string, el: HTMLElement) => {
      clicked.push(what);
      await user.click(el);
    };

    // Click 5 of the live count, "Attach in Data" on the classification's page, records the classification.
    setAttachIntent("t-uxwalk");
    render(
      <AuthContext.Provider value={{ me: STEWARD, isLoading: false }}>
        <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
          <TooltipProvider>
            <MemoryRouter initialEntries={["/governance/data/ice/sales/orders?column=email"]}>
              <Routes>
                <Route path="/governance/data/*" element={<DataRoutes />} />
              </Routes>
            </MemoryRouter>
          </TooltipProvider>
        </QueryClientProvider>
      </AuthContext.Provider>,
    );
    const panel = await screen.findByRole("dialog", { name: "email" });

    // Attach: the dialog opens on the classification the user came to attach.
    await click("Add classification (column panel)", await within(panel).findByRole("button", { name: "Add classification" }));
    const attach = await screen.findByRole("dialog", { name: "Add classification" });
    await waitFor(() => expect(within(attach).getByRole("combobox", { name: "Classification" })).toHaveTextContent("uxwalk-pii"));
    data.tags = { "c-ice|sales|orders": ok(ON_EMAIL) };
    await click("Add classification (dialog)", within(attach).getByRole("button", { name: "Add classification" }));
    await waitFor(() => expect(tagWrites).toHaveLength(1));
    await waitFor(() => expect(within(panel).getByRole("list", { name: "Classifications of email" })).toHaveTextContent("uxwalk-pii"));

    // Rule: it starts on the column's classification.
    await click("Add rule (column panel)", within(panel).getByRole("button", { name: "Add rule" }));
    const rule = await screen.findByRole("dialog", { name: "New rule" });
    expect(within(rule).getByRole("combobox", { name: "Applies to" })).toHaveTextContent("Classification");

    // Exempt: one search over users and groups, no kind switch.
    await click("Exempt search", within(rule).getByRole("combobox", { name: "Exempt search" }));
    await user.type(within(rule).getByRole("combobox", { name: "Exempt search" }), "data");
    await click("data-team", await within(rule).findByRole("option", { name: /^data-team/ }));

    await click("Preview", within(rule).getByRole("button", { name: "Preview" }));
    const create = within(rule).getByRole("button", { name: "Create" });
    await waitFor(() => expect(create).toHaveAttribute("aria-disabled", "false"));
    expect(rule).toHaveTextContent("Not enforced yet.");
    await click("Create", create);

    await waitFor(() => expect(permCalls.some((c) => c.call === "POST /perm/rules")).toBe(true));
    expect(permCalls.find((c) => c.call === "POST /perm/rules")?.body).toEqual({
      name: "Mask uxwalk-pii",
      kind: { type: "mask", mask_type: "MASK" },
      selector: { type: "tag", key: "uxwalk-pii" },
      exempt: { include: [{ kind: "group", ref: "data-team" }], exclude: [] },
    });
    expect(clicked).toEqual([
      "Add classification (column panel)",
      "Add classification (dialog)",
      "Add rule (column panel)",
      "Exempt search",
      "data-team",
      "Preview",
      "Create",
    ]);
    // 8 clicks reach the column panel (live count, unchanged pages) + these.
    expect(8 + clicked.length).toBeLessThanOrEqual(15);
  });
});
