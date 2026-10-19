import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ExplainCapabilityResponse } from "@/api/generated/model";
import {
  CAPABILITIES,
  E_TR,
  engineGrant,
  grantsResponse,
  managedEntry,
  ok,
  openArea,
  policy,
  profileDto,
  READER,
  serve,
  type Answer,
} from "./engineHarness";

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, "scrollIntoView").mockImplementation(() => undefined);
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const T = { timeout: 3000 };

const REACH = { engines: [{ id: E_TR, name: "lake-trino", origin: "provisioned" as const }], sharedProfile: false };
const ENGINE = { id: E_TR, name: "lake-trino", type: "trino", origin: "provisioned", status: "running" } as const;

function explain(over: Partial<ExplainCapabilityResponse>): ExplainCapabilityResponse {
  return {
    engine: ENGINE,
    profile: { serviceId: 2, name: "lake-tr" },
    reach: REACH,
    outcome: "per_profile",
    decision: null,
    attribution: null,
    policyName: null,
    tagResolution: "not_applicable",
    ...over,
  };
}

function answers(result: ExplainCapabilityResponse, over: Record<string, Answer> = {}): Record<string, Answer> {
  return {
    [`/perm/engines/${E_TR}/grants`]: ok(
      grantsResponse({
        grants: [
          engineGrant({
            id: "g-imp-ub",
            capability: "trino.impersonate_user",
            values: ["ub"],
            principals: [{ kind: "user", ref: "ua" }],
          }),
        ],
        managed: [managedEntry({ policyId: 11, kind: "delegator", policyName: "egret-delegator-impersonate" })],
      }),
    ),
    "/perm/engine-capabilities": ok(CAPABILITIES),
    "/governance/catalogs": ok([]),
    "/policies/services": ok([profileDto("lake-tr")]),
    "/policies?service=lake-tr": ok([policy(1, "compiler")]),
    "POST /perm/explain/capability": ok(result),
    ...over,
  };
}

async function ask(result: ExplainCapabilityResponse) {
  const served = serve(answers(result));
  openArea(`/governance/engines/${E_TR}`, READER);
  await userEvent.click(await screen.findByRole("button", { name: "Check" }, T));
  const dialog = await screen.findByRole("dialog", { name: "Check an engine permission" });
  await userEvent.type(within(dialog).getByLabelText("User"), "ua");
  await userEvent.click(within(dialog).getByRole("combobox", { name: "Capability" }));
  await userEvent.click(await screen.findByRole("option", { name: /^Impersonate a user/ }));
  await userEvent.type(within(dialog).getByLabelText("Target user"), "ub");
  await userEvent.click(within(dialog).getByRole("button", { name: "Check" }));
  const answer = await within(dialog).findByRole("region", { name: "Answer" }, T);
  return { served, dialog, answer };
}

describe("Check (spec §6.1)", () => {
  it("asks explain/capability with the user, the capability and the values", async () => {
    const { served } = await ask(
      explain({ decision: { kind: "allowed", policy_name: "perm-engine-g-imp-ub" }, policyName: "perm-engine-g-imp-ub" }),
    );
    expect(served.bodies["POST /perm/explain/capability"]).toEqual([
      { subject: { user: "ua" }, engineId: E_TR, capability: "trino.impersonate_user", values: ["ub"] },
    ]);
  });

  it("Allowed by an engine grant: word, icon, the deciding policy and a link to its row", async () => {
    const { answer } = await ask(
      explain({
        decision: { kind: "allowed", policy_name: "perm-engine-g-imp-ub" },
        attribution: { kind: "engine_grant", engine_grant_id: "g-imp-ub" },
        policyName: "perm-engine-g-imp-ub",
      }),
    );
    const verdict = within(answer).getByText("Allowed");
    expect(verdict.closest("p")!.querySelector("svg")).not.toBeNull();
    expect(within(answer).getByText("perm-engine-g-imp-ub")).toBeInTheDocument();
    expect(within(answer).getByRole("link", { name: "egret engine grant" })).toHaveAttribute(
      "href",
      `/governance/engines/${E_TR}?grant=g-imp-ub`,
    );
  });

  it("Denied with an empty policy name names no deciding policy", async () => {
    const { answer } = await ask(
      explain({
        decision: { kind: "denied", policy_name: "", reason: "no_policy_matched" },
        policyName: "",
      }),
    );
    const verdict = within(answer).getByText("Denied");
    expect(verdict.closest("p")!.querySelector("svg")).not.toBeNull();
    expect(within(answer).queryByText(/Decided by/)).not.toBeInTheDocument();
    expect(within(answer).getByText("No policy grants it.")).toBeInTheDocument();
  });

  it("Undecidable on a delegating Trino profile: the words and the impersonation sentence", async () => {
    const { answer } = await ask(
      explain({
        decision: {
          kind: "undecidable",
          why: [
            {
              kind: "policy_construct",
              policy_name: "egret-user-impersonate-self",
              policy_id: 7,
              construct: {
                kind: "resource_value_expression_marker",
                resource_type: "trinouser",
                value: "{USER}",
              },
            },
          ],
        } as ExplainCapabilityResponse["decision"],
      }),
    );
    const verdict = within(answer).getByText("Cannot be decided ahead of time");
    expect(verdict.closest("p")!.querySelector("svg")).not.toBeNull();
    expect(
      within(answer).getByText(
        "egret cannot decide impersonation on this engine: its baseline lets every user become themself through {USER}, which egret's evaluator does not expand.",
      ),
    ).toBeInTheDocument();
    expect(within(answer).getAllByRole("listitem").length).toBeGreaterThanOrEqual(1);
  });

  it.each([
    ["egret-seeded", { policyName: "egret-delegator-impersonate", attribution: null }],
    ["egret-compiled", { policyName: "perm-grant-0b8f2c1e-1111-4222-8333-444455556666", attribution: { kind: "grant", grant_id: "0b8f2c1e-1111-4222-8333-444455556666" } }],
    ["hand-written", { policyName: "ops can become anyone", attribution: null }],
  ] as const)("names the deciding policy's source: %s", async (source, over) => {
    const { answer } = await ask(
      explain({
        decision: { kind: "allowed", policy_name: over.policyName },
        policyName: over.policyName,
        attribution: over.attribution as ExplainCapabilityResponse["attribution"],
      }),
    );
    expect(within(answer).getByText(source)).toBeInTheDocument();
  });

  it("no profile and declined tags have their sentences", async () => {
    const { answer } = await ask(explain({ outcome: "no_profile", profile: null }));
    expect(within(answer).getByText("This engine has no profile to ask.")).toBeInTheDocument();
    vi.unstubAllGlobals();
  });

  it("says when egret cannot resolve classification tags", async () => {
    const { answer } = await ask(
      explain({
        decision: { kind: "undecidable", why: [{ kind: "tags_unresolved" }] } as ExplainCapabilityResponse["decision"],
        tagResolution: "declined",
      }),
    );
    expect(
      within(answer).getByText("egret cannot resolve classification tags for this question."),
    ).toBeInTheDocument();
  });

  it("focuses the user field; Escape closes its suggestions, then the dialog", async () => {
    serve(answers(explain({})));
    openArea(`/governance/engines/${E_TR}`, READER);
    await userEvent.click(await screen.findByRole("button", { name: "Check" }, T));
    const dialog = await screen.findByRole("dialog", { name: "Check an engine permission" });
    await waitFor(() => expect(within(dialog).getByLabelText("User")).toHaveFocus());
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(within(dialog).getByLabelText("User")).toHaveAttribute("aria-expanded", "false"));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    // Live check L8: focus returns to the button that opened the dialog, not to the page body.
    await waitFor(() => expect(screen.getByRole("button", { name: "Check" })).toHaveFocus());
  });
});
