import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { HandWrittenPolicy, ItemSummary, ObjectHandWritten } from "@/api/generated/model";
import type { Me } from "@/api/hooks";
import { AuthContext } from "@/auth/useAuth";
import { fail, me, ok, serve, type Answer } from "@/routes/governance/engines/__tests__/engineHarness";
import { HandWrittenSection, type HandWrittenShow } from "@/routes/governance/handwritten/HandWrittenSection";

afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const READER = me(["policy:read"]);
const KEY = "GET /governance/objects/c-ice/handwritten?schema=sales&table=orders";

const item = (over: Partial<ItemSummary> & Pick<ItemSummary, "bucket">): ItemSummary => ({
  users: [],
  groups: [],
  roles: [],
  everyone: false,
  accesses: [],
  ...over,
});

function policy(over: Partial<HandWrittenPolicy> & Pick<HandWrittenPolicy, "id" | "name">): HandWrittenPolicy {
  return {
    service: "lake-tr",
    inTagService: false,
    policyType: "access",
    match: "applies",
    throughPattern: false,
    viaTag: null,
    resourceSummary: "catalog=ice; schema=sales; table=orders",
    enabled: true,
    denyAllElse: false,
    unmodelled: [],
    items: [item({ bucket: "allow", users: ["ua"], accesses: ["select"] })],
    parseError: null,
    ...over,
  };
}

function answer(over: Partial<ObjectHandWritten> = {}): ObjectHandWritten {
  return {
    catalog: { id: "c-ice", name: "ice" },
    target: { schema: "sales", table: "orders", column: null },
    state: "listed",
    reason: null,
    profiles: [
      {
        serviceId: 2,
        name: "lake-tr",
        type: "trino",
        tagService: "lake-tr-tag",
        engines: [{ id: "e-tr", name: "lake-trino", type: "trino", origin: "provisioned" }],
      },
      { serviceId: 3, name: "spare-sr", type: "starrocks", tagService: null, engines: [] },
    ],
    policies: [],
    seeded: [],
    ...over,
  };
}

function open(a: Answer, as: Me = READER, show: HandWrittenShow = "access", column?: string) {
  const served = serve({ [column ? `${KEY}&column=${column}` : KEY]: a });
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me: as, isLoading: false }}>
        <MemoryRouter>
          <HandWrittenSection catalogId="c-ice" schema="sales" table="orders" column={column} show={show} />
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  return served;
}

const section = () => screen.getByRole("region", { name: /^Manual policies/ });
const rowOf = (name: string) => within(section()).getByRole("link", { name }).closest("tr") as HTMLElement;

describe("HandWrittenSection", () => {
  it("lists rows by match level, each badged Manual with its level word (Q2)", async () => {
    open(
      ok(
        answer({
          policies: [
            policy({ id: 4, name: "p-unknown", match: "unknown", parseError: "expected value at line 1", items: [] }),
            policy({ id: 3, name: "p-parent", match: "parent", resourceSummary: "catalog=ice; schema=sales" }),
            policy({ id: 2, name: "p-part", match: "part" }),
            policy({ id: 1, name: "p-applies", match: "applies", throughPattern: true }),
          ],
        }),
      ),
    );
    await within(await screen.findByRole("region", { name: "Manual policies (4)" }, T)).findByRole("link", { name: "p-applies" });
    expect(screen.queryByText(/hand-written/i)).not.toBeInTheDocument();
    const names = within(section()).getAllByRole("row").slice(1).map((r) => within(r).getAllByRole("link")[0]?.textContent);
    expect(names).toEqual(["p-applies", "p-part", "p-parent", "p-unknown"]);
    expect(rowOf("p-applies")).toHaveTextContent("Manual");
    expect(rowOf("p-applies")).toHaveTextContent("Applies to this object · through a pattern");
    expect(rowOf("p-part")).toHaveTextContent("Applies to part of it");
    expect(rowOf("p-parent")).toHaveTextContent("Set on a parent");
    expect(rowOf("p-parent")).toHaveTextContent("Names catalog=ice; schema=sales only; engines do not apply it to this object's data.");
    expect(rowOf("p-unknown")).toHaveTextContent("Could not be matched: expected value at line 1");
  });

  it("summarises items: deny marked, everyone, group and role reach nobody (Q1)", async () => {
    open(
      ok(
        answer({
          policies: [
            policy({
              id: 1,
              name: "p1",
              items: [
                item({ bucket: "deny", users: ["ua"], accesses: ["select"] }),
                item({ bucket: "allow", everyone: true, accesses: ["select"] }),
                item({ bucket: "allow", groups: [{ name: "g1", onEngines: false }], roles: [{ name: "r1", onEngines: false }], accesses: ["select"] }),
              ],
            }),
          ],
        }),
      ),
    );
    await screen.findByRole("link", { name: "p1" }, T);
    const items = within(rowOf("p1")).getByRole("list", { name: "Items" });
    const lines = within(items).getAllByRole("listitem");
    expect(lines[0]).toHaveTextContent("Deny: ua — select");
    expect(lines[0].querySelector("svg")).not.toBeNull();
    expect(lines[1]).toHaveTextContent("Allow: Everyone — select");
    expect(lines[1].querySelector("svg")).toBeNull();
    expect(lines[2]).toHaveTextContent("Names group g1. Engines resolve no groups, so on the engine it reaches nobody.");
    expect(lines[2]).toHaveTextContent("Names role r1. Engines resolve no roles, so on the engine it reaches nobody.");
  });

  it("marks a disabled policy and states deny all else and unmodelled constructs", async () => {
    open(ok(answer({ policies: [policy({ id: 1, name: "p1", enabled: false, denyAllElse: true, unmodelled: ["Has conditions egret does not evaluate."] })] })));
    await screen.findByRole("link", { name: "p1" }, T);
    expect(rowOf("p1")).toHaveTextContent("Disabled");
    expect(rowOf("p1")).toHaveTextContent("Denies every access its items do not allow (deny all else).");
    expect(rowOf("p1")).toHaveTextContent("Has conditions egret does not evaluate.");
  });

  it("links each row to its engine's Advanced editor; a tag-service row opens the By-tag editor", async () => {
    open(
      ok(
        answer({
          policies: [
            policy({ id: 7, name: "res" }),
            policy({ id: 8, name: "tagged", service: "lake-tr-tag", inTagService: true, policyType: "masking", viaTag: { name: "PII", on: "below", schema: "sales", table: "orders", column: "email" }, match: "part" }),
            policy({ id: 9, name: "sr", service: "spare-sr" }),
          ],
        }),
      ),
      READER,
      "all",
    );
    await screen.findByRole("link", { name: "res" }, T);
    expect(within(section()).getByRole("link", { name: "res" })).toHaveAttribute("href", "/governance/engines/e-tr/advanced/policies/access/7");
    expect(within(section()).getByRole("link", { name: "tagged" })).toHaveAttribute(
      "href",
      "/governance/engines/e-tr/advanced/policies/masking/8?scope=tag",
    );
    expect(rowOf("tagged")).toHaveTextContent("Through tag PII on column email");
    // An unbound profile: its own page.
    expect(within(section()).getByRole("link", { name: "sr" })).toHaveAttribute(
      "href",
      "/governance/engines/profiles/spare-sr/advanced/policies/access/9",
    );
  });

  it.each([
    // [page target, viaTag, sentence]: a tag below the object names where it sits, relative to the page; never "parent".
    [{ schema: "sales", table: "orders", column: null }, { on: "below", schema: "sales", table: "orders", column: "email" }, "Through tag PII on column email"],
    [{ schema: "sales", table: null, column: null }, { on: "below", schema: "sales", table: "orders", column: "email" }, "Through tag PII on column orders.email"],
    [{ schema: "sales", table: null, column: null }, { on: "below", schema: "sales", table: "orders", column: null }, "Through tag PII on table orders"],
    [{ schema: null, table: null, column: null }, { on: "below", schema: "sales", table: "orders", column: null }, "Through tag PII on table sales.orders"],
    [{ schema: null, table: null, column: null }, { on: "below", schema: "sales", table: "orders", column: "email" }, "Through tag PII on column sales.orders.email"],
    [{ schema: "sales", table: "orders", column: "email" }, { on: "parent", schema: "sales", table: "orders", column: null }, "Through tag PII on table orders, which holds this column"],
    [{ schema: "sales", table: "orders", column: null }, { on: "this", schema: "sales", table: "orders", column: null }, "Through tag PII on this object"],
    [{ schema: "sales", table: "orders", column: "email" }, { on: "this", schema: "sales", table: "orders", column: "email" }, "Through tag PII on this object"],
  ] as const)("names where the matched tag sits (%j, %j)", async (target, via, sentence) => {
    open(
      ok(
        answer({
          target,
          policies: [
            policy({ id: 8, name: "tagged", service: "lake-tr-tag", inTagService: true, viaTag: { name: "PII", ...via }, match: via.on === "below" ? "part" : "applies" }),
          ],
        }),
      ),
    );
    await screen.findByRole("link", { name: "tagged" }, T);
    const reach = within(rowOf("tagged")).getByText(/^Through tag/);
    expect(reach).toHaveTextContent(new RegExp(`^${sentence}$`));
    expect(rowOf("tagged")).not.toHaveTextContent(/on a parent/);
  });

  it("shows only access rows for access, masking and row-filter rows for rules, every row for all", async () => {
    const policies = [
      policy({ id: 1, name: "acc" }),
      policy({ id: 2, name: "msk", policyType: "masking" }),
      policy({ id: 3, name: "rf", policyType: "rowfilter" }),
    ];
    open(ok(answer({ policies })), READER, "access");
    await screen.findByRole("link", { name: "acc" }, T);
    expect(within(section()).queryByRole("link", { name: "msk" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Manual policies (1)" })).toBeInTheDocument();
  });

  it.each([["rules" as const], ["all" as const]])("show=%s", async (show) => {
    const policies = [
      policy({ id: 1, name: "acc" }),
      policy({ id: 2, name: "msk", policyType: "masking" }),
      policy({ id: 3, name: "rf", policyType: "rowfilter" }),
    ];
    open(ok(answer({ policies })), READER, show);
    await screen.findByRole("link", { name: "msk" }, T);
    expect(within(section()).getByRole("link", { name: "rf" })).toBeInTheDocument();
    expect(!!within(section()).queryByRole("link", { name: "acc" })).toBe(show === "all");
  });

  it("asks route A for the column when given one", async () => {
    const served = open(ok(answer()), READER, "all", "email");
    await screen.findByText("No manual policy reaches this object.", {}, T);
    expect(served.asked).toContain(`${KEY}&column=email`);
  });

  it("counts egret-seeded policies, linking each engine's Policies tab", async () => {
    open(ok(answer({ seeded: [{ service: "lake-tr", count: 3 }, { service: "spare-sr", count: 1 }] })));
    const line = await screen.findByText(/egret-seeded policies also reach this object/, {}, T);
    expect(line).toHaveTextContent("4 egret-seeded policies also reach this object; see Engine & policy:");
    expect(within(line).getByRole("link", { name: "lake-trino" })).toHaveAttribute("href", "/governance/engines/e-tr?tab=policies");
    expect(within(line).getByRole("link", { name: "spare-sr" })).toHaveAttribute(
      "href",
      "/governance/engines/profiles/spare-sr?tab=policies",
    );
  });

  it("links a tag service's seeded count to the engines (or profile) whose tag service it is, never a page named after it", async () => {
    open(
      ok(
        answer({
          profiles: [
            ...answer().profiles,
            { serviceId: 4, name: "spare-tr", type: "trino", tagService: "spare-tr-tag", engines: [] },
          ],
          seeded: [
            { service: "lake-tr-tag", count: 2 },
            { service: "spare-tr-tag", count: 1 },
            { service: "lost-tag", count: 1 },
          ],
        }),
      ),
    );
    const line = await screen.findByText(/egret-seeded policies also reach this object/, {}, T);
    expect(line).toHaveTextContent("4 egret-seeded policies also reach this object; see Engine & policy:");
    expect(within(line).getByRole("link", { name: "lake-trino" })).toHaveAttribute("href", "/governance/engines/e-tr?tab=policies");
    expect(within(line).getByRole("link", { name: "spare-tr" })).toHaveAttribute(
      "href",
      "/governance/engines/profiles/spare-tr?tab=policies",
    );
    // A service no listed profile names: its name, no link.
    expect(line).toHaveTextContent("lost-tag");
    expect(within(line).queryByRole("link", { name: "lost-tag" })).not.toBeInTheDocument();
    for (const a of within(line).getAllByRole("link")) {
      expect(a.getAttribute("href")).not.toMatch(/-tag/);
    }
  });

  it.each([
    ["no_engine_attached", "No engine is attached to this catalog, so no manual policy reaches it."],
    ["no_profile", "The engines attached to this catalog have no policy profile, so no manual policy reaches it."],
    [
      "catalog_not_in_profile",
      "No policy profile of the attached engines lists this catalog, so manual policies could not be matched to it.",
    ],
  ] as const)("not_determined %s says why", async (reason, sentence) => {
    open(ok(answer({ state: "not_determined", reason, profiles: [] })));
    expect(await screen.findByText(sentence, {}, T)).toBeInTheDocument();
    expect(screen.queryByText("No manual policy reaches this object.")).not.toBeInTheDocument();
  });

  it("explains, and asks nothing, without policy:read", async () => {
    const served = open(ok(answer()), me(["tag:read"]));
    expect(
      await screen.findByText(
        "Manual policies are read with the policy:read permission, which this account does not have.",
        {},
        T,
      ),
    ).toBeInTheDocument();
    expect(served.asked).toEqual([]);
  });

  it("shows a failure with Retry, never None", async () => {
    const user = userEvent.setup();
    const served = open(fail(500));
    expect(await screen.findByText(/The manual policies could not be loaded/, {}, T)).toBeInTheDocument();
    expect(screen.queryByText("No manual policy reaches this object.")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(served.asked.filter((a) => a === KEY)).toHaveLength(2));
  });

  it("says so when none reaches the object", async () => {
    open(ok(answer()));
    expect(await screen.findByText("No manual policy reaches this object.", {}, T)).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Manual policies (0)" })).toBeInTheDocument();
  });
});
