import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ItemSummary, TagHandWritten, TagPolicyRow } from "@/api/generated/model";
import { fail, ok, openArea, serve, STEWARD, tag, TAG_POLICY_READER, TAG_READER, type Answer } from "./harness";

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

describe("Mask & filter tab", () => {
  it("lists the rules on this tag for a policy:read holder", async () => {
    open("rules", TAG_POLICY_READER, { "/perm/rules": ok([]) });
    expect(await screen.findByRole("heading", { name: "Mask and filter rules" }, T)).toBeInTheDocument();
    expect(await screen.findByText("No egret rule selects this tag.", {}, T)).toBeInTheDocument();
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

describe("View exposure tab", () => {
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
  return { tag: { id: "t-pii", key: "pii", name: "PII" }, state: "listed", policies: [], sameName: [], unreadable: [], ...over };
}

const rowOf = (name: string) => screen.getByRole("link", { name }).closest("tr") as HTMLElement;

describe("Hand-written tab (route D)", () => {
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

  it("says when another tag has this name (CL10)", async () => {
    open("handwritten", STEWARD, {
      [HW]: ok(handwritten({ policies: [row({ id: 1, name: "p1" })], sameName: [{ id: "t-2", key: "pii2" }] })),
    });
    const line = await screen.findByText(
      (_c, el) => el?.tagName === "P" && /^Tag pii2 has this name too/.test(el.textContent ?? ""),
      {},
      T,
    );
    expect(line).toHaveTextContent("Tag pii2 has this name too; engine tag policies on this name apply to both.");
    expect(within(line).getByRole("link", { name: "pii2" })).toHaveAttribute("href", "/governance/classifications/t-2");
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
    expect(note).toHaveTextContent("1 hand-written tag policy could not be read, so this list may be incomplete.");
    expect(within(note).getByRole("link", { name: "broken" })).toHaveAttribute(
      "href",
      "/governance/engines/e-1/advanced/policies/masking/51?scope=tag",
    );
    expect(note).toHaveTextContent("invalid type: string");
    // Not "none": the unreadable policy may name this classification.
    expect(screen.queryByText("No hand-written tag policy names this classification.")).not.toBeInTheDocument();
    expect(screen.getByText("No readable hand-written tag policy names this classification.")).toBeInTheDocument();
  });

  it("says when no profile has a tag service", async () => {
    open("handwritten", STEWARD, { [HW]: ok(handwritten({ state: "no_tag_service" })) });
    expect(
      await screen.findByText("No engine profile has a tag service, so no engine downloads tag policies.", {}, T),
    ).toBeInTheDocument();
    expect(screen.queryByText("No hand-written tag policy names this classification.")).not.toBeInTheDocument();
  });

  it("says so when no hand-written tag policy names it", async () => {
    open("handwritten", STEWARD, { [HW]: ok(handwritten()) });
    expect(await screen.findByText("No hand-written tag policy names this classification.", {}, T)).toBeInTheDocument();
  });

  it("explains, and asks nothing, without policy:read", async () => {
    const served = open("handwritten", TAG_READER);
    expect(
      await screen.findByText(
        "Hand-written tag policies are read with the policy:read permission, which this account does not have.",
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
      await screen.findByText(/The hand-written tag policies could not be loaded/, {}, T),
    ).toBeInTheDocument();
    expect(screen.queryByText("No hand-written tag policy names this classification.")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(served.asked.filter((a) => a === `GET ${HW}`)).toHaveLength(2));
  });
});
