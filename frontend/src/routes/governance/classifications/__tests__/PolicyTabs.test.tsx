import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ItemSummary, TagHandWritten, TagPolicyRow } from "@/api/generated/model";
import { ADMIN, fail, ok, openArea, serve, STEWARD, tag, TAG_POLICY_READER, TAG_READER, type Answer } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const PII = tag({ id: "t-pii", key: "pii", name: "PII" });
const BASE = "/governance/classifications/t-pii";
const HW = "/classifications/t-pii/handwritten";

function open(tab: string, as = STEWARD, extra: Record<string, Answer> = {}) {
  const served = serve({ "/tags": ok([PII]), ...extra });
  openArea(`${BASE}?tab=${tab}`, as);
  return served;
}

describe("Rules tab", () => {
  it("lists the rules on this classification for a policy:read holder", async () => {
    open("rules", TAG_POLICY_READER, { "/perm/rules": ok([]) });
    expect(await screen.findByRole("heading", { name: "Rules" }, T)).toBeInTheDocument();
    const rules = screen.getByRole("region", { name: "Rules" });
    expect(await within(rules).findByText("No rule uses this classification yet.", {}, T)).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Mask and filter rules" })).not.toBeInTheDocument();
    expect(within(rules).getAllByText("Adding a rule needs the policy:update permission.")).toHaveLength(1);
    expect(within(rules).queryByRole("button", { name: "Add rule" })).not.toBeInTheDocument();
    expect(screen.queryByText(/egret rule/)).not.toBeInTheDocument();
  });

  it("offers Add rule in the toolbar once rules exist", async () => {
    open("rules", ADMIN, {
      "/perm/rules": ok([
        { id: "r1", name: "Hash email", kind: { type: "mask", mask_type: "MASK_HASH" }, selector: { type: "tag", key: "pii" }, exempt: { include: [], exclude: [] }, tag_name: "PII" },
      ]),
    });
    const rules = await screen.findByRole("region", { name: "Rules" }, T);
    await within(rules).findByRole("link", { name: "Hash email" }, T);
    expect(within(rules).getAllByRole("button", { name: "Add rule" })).toHaveLength(1);
  });

  it("offers Add rule in the empty state with policy:update, on this classification", async () => {
    const user = userEvent.setup();
    open("rules", ADMIN, { "/perm/rules": ok([]), "/governance/catalogs": ok([]) });
    const rules = await screen.findByRole("region", { name: "Rules" }, T);
    await within(rules).findByText("No rule uses this classification yet.", {}, T);
    // While the list is empty, the empty state carries the one Add rule (no toolbar duplicate).
    const adds = within(rules).getAllByRole("button", { name: "Add rule" });
    expect(adds).toHaveLength(1);
    await user.click(adds[0]);
    const dialog = await screen.findByRole("dialog", { name: "New rule" });
    expect(within(dialog).getByLabelText(/^Name/)).toHaveValue("Mask PII");
  });

  it("explains, and asks for no rules, without policy:read", async () => {
    const served = open("rules", TAG_READER);
    expect(
      await screen.findByText(
        "Rules are read with the policy:read permission, which this account does not have.",
        {},
        T,
      ),
    ).toBeInTheDocument();
    expect(served.asked).not.toContain("GET /perm/rules");
  });
});

describe("Views tab", () => {
  it("shows the materialized views section", async () => {
    open("views", TAG_READER, { "/classifications/t-pii/view-exposure": ok({ starrocksMaterializedViews: [], enginesNotChecked: [] }) });
    expect(await screen.findByRole("heading", { name: "Existing materialized views" }, T)).toBeInTheDocument();
  });
});

const item = (over: Partial<ItemSummary> & Pick<ItemSummary, "bucket">): ItemSummary => ({
  users: [],
  groups: [],
  roles: [],
  everyone: false,
  accesses: [],
  ...over,
});

function row(over: Partial<TagPolicyRow> & Pick<TagPolicyRow, "id" | "name">): TagPolicyRow {
  return {
    service: "lake-tr-tag",
    policyType: "access",
    resourceSummary: "tag=PII",
    throughPattern: false,
    enabled: true,
    denyAllElse: false,
    unmodelled: [],
    items: [],
    profiles: [{ serviceId: 2, name: "lake-tr" }],
    engines: [{ id: "e-1", name: "lake-trino" }],
    ...over,
  };
}

function handwritten(over: Partial<TagHandWritten> = {}): TagHandWritten {
  return { tag: { id: "t-pii", key: "pii", name: "PII" }, state: "listed", policies: [], unreadable: [], ...over };
}

const rowOf = (name: string) => screen.getByRole("link", { name }).closest("tr") as HTMLElement;

describe("Manual policies tab (route D)", () => {
  it("lists each policy with its editor link, engines, type and items", async () => {
    open("handwritten", STEWARD, {
      [HW]: ok(
        handwritten({
          policies: [
            row({
              id: 41,
              name: "deny-pii",
              items: [item({ bucket: "deny", users: ["ua"], accesses: ["select"] })],
            }),
            row({
              id: 42,
              name: "mask-pii",
              policyType: "masking",
              engines: [],
              profiles: [{ serviceId: 3, name: "spare-tr" }],
              items: [item({ bucket: "mask", everyone: true, maskType: "MASK_NULL" })],
            }),
          ],
        }),
      ),
    });
    await screen.findByRole("link", { name: "deny-pii" }, T);
    expect(screen.getByRole("link", { name: "deny-pii" })).toHaveAttribute(
      "href",
      "/governance/engines/e-1/advanced/policies/access/41?scope=tag",
    );
    expect(rowOf("deny-pii")).toHaveTextContent("lake-trino");
    expect(rowOf("deny-pii")).toHaveTextContent("Access");
    expect(within(rowOf("deny-pii")).getByRole("list", { name: "Items" })).toHaveTextContent("Deny: ua — select");
    // An unbound profile: its own page's editor.
    expect(screen.getByRole("link", { name: "mask-pii" })).toHaveAttribute(
      "href",
      "/governance/engines/profiles/spare-tr/advanced/policies/masking/42?scope=tag",
    );
    expect(rowOf("mask-pii")).toHaveTextContent("No engine downloads profile spare-tr");
    expect(rowOf("mask-pii")).toHaveTextContent("Mask: Everyone — mask MASK_NULL");
  });

  it("marks group and role items as reaching nobody on the engine (Q1)", async () => {
    open("handwritten", STEWARD, {
      [HW]: ok(
        handwritten({
          policies: [
            row({
              id: 1,
              name: "p1",
              items: [
                item({
                  bucket: "allow",
                  groups: [{ name: "g1", onEngines: false }],
                  roles: [{ name: "r1", onEngines: false }],
                  accesses: ["select"],
                }),
              ],
            }),
          ],
        }),
      ),
    });
    await screen.findByRole("link", { name: "p1" }, T);
    expect(rowOf("p1")).toHaveTextContent("Names group g1. Engines resolve no groups, so on the engine it reaches nobody.");
    expect(rowOf("p1")).toHaveTextContent("Names role r1. Engines resolve no roles, so on the engine it reaches nobody.");
  });

  it("states disabled, deny all else, unmodelled constructs and a pattern match", async () => {
    open("handwritten", STEWARD, {
      [HW]: ok(
        handwritten({
          policies: [
            row({
              id: 1,
              name: "p1",
              enabled: false,
              denyAllElse: true,
              throughPattern: true,
              unmodelled: ["Has conditions egret does not evaluate."],
            }),
          ],
        }),
      ),
    });
    await screen.findByRole("link", { name: "p1" }, T);
    const r = rowOf("p1");
    expect(r).toHaveTextContent("Disabled");
    expect(r).toHaveTextContent("Denies every access its items do not allow (deny all else).");
    expect(r).toHaveTextContent("Has conditions egret does not evaluate.");
    expect(r).toHaveTextContent("Names this classification through a pattern.");
  });

  it("lists unreadable tag policies and says the list may be incomplete", async () => {
    open("handwritten", STEWARD, {
      [HW]: ok(
        handwritten({
          unreadable: [
            {
              id: 51,
              name: "broken",
              service: "lake-tr-tag",
              inTagService: true,
              policyType: "masking",
              parseError: "invalid type: string",
              engines: [{ id: "e-1", name: "lake-trino" }],
            },
          ],
        }),
      ),
    });
    const note = await screen.findByRole("region", { name: "Could not be read" }, T);
    expect(note).toHaveTextContent("1 manual tag policy could not be read, so this list may be incomplete.");
    expect(within(note).getByRole("link", { name: "broken" })).toHaveAttribute(
      "href",
      "/governance/engines/e-1/advanced/policies/masking/51?scope=tag",
    );
    expect(note).toHaveTextContent("invalid type: string");
    // Not "none": the unreadable policy may name this classification.
    expect(screen.queryByText("No manual tag policy names this classification.")).not.toBeInTheDocument();
    expect(screen.getByText("No readable manual tag policy names this classification.")).toBeInTheDocument();
  });

  it("says when no profile has a tag service", async () => {
    open("handwritten", STEWARD, { [HW]: ok(handwritten({ state: "no_tag_service" })) });
    expect(
      await screen.findByText("No engine profile has a tag service, so no engine downloads tag policies.", {}, T),
    ).toBeInTheDocument();
    expect(screen.queryByText("No manual tag policy names this classification.")).not.toBeInTheDocument();
  });

  it("says so when no manual tag policy names it", async () => {
    open("handwritten", STEWARD, { [HW]: ok(handwritten()) });
    expect(await screen.findByText("No manual tag policy names this classification.", {}, T)).toBeInTheDocument();
    expect(screen.queryByText(/hand-written/i)).not.toBeInTheDocument();
  });

  it("explains, and asks nothing, without policy:read", async () => {
    const served = open("handwritten", TAG_READER);
    expect(
      await screen.findByText(
        "Manual tag policies are read with the policy:read permission, which this account does not have.",
        {},
        T,
      ),
    ).toBeInTheDocument();
    expect(served.asked).not.toContain(`GET ${HW}`);
  });

  it("shows a failure with Retry, never the empty sentence", async () => {
    const user = userEvent.setup();
    const served = open("handwritten", STEWARD, { [HW]: fail(500) });
    expect(
      await screen.findByText(/The manual tag policies could not be loaded/, {}, T),
    ).toBeInTheDocument();
    expect(screen.queryByText("No manual tag policy names this classification.")).not.toBeInTheDocument();
    await user.click(within(screen.getByRole("tabpanel")).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(served.asked.filter((a) => a === `GET ${HW}`)).toHaveLength(2));
  });
});
