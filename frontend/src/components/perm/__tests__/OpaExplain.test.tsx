import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, test } from "vitest";

/**
 * Governance SP2b Task 16 (review I3): a profile on OPA is answered by egret's
 * own evaluator, so its verdicts carry `ir_reason` and `opa` values instead of
 * a Ranger policy. The page must not call them "outside the permission model"
 * or "Ranger policy …, id unknown", and the group view must count an
 * OPA-masked member as masked.
 */
import type { ExplainPrincipalResponse, ExplainProfile, ExplainResponse } from "@/api/generated/model";
import { ExplainResultView } from "@/components/perm/ExplainResult";
import { MembersExplainView } from "@/components/perm/MembersExplainView";

const MASK = "cast(to_hex(sha256(to_utf8(email))) as varchar)";

function opaProfile(over: Partial<ExplainProfile> = {}): ExplainProfile {
  return {
    service_id: 21,
    service_name: "trino-opa",
    engine: "trino",
    mechanism: "opa",
    tags: [],
    tag_policies_in_download: 0,
    effect_lines: [],
    accesses: [
      {
        access_type: "select",
        decision: { kind: "allowed", policy_id: null, policy_name: "grant 6f1c" },
        attribution: null,
        ir_reason: "grant 6f1c",
      },
    ],
    opa: { column_mask: MASK, column_mask_type: "MASK_HASH", row_filter: "(region = 'eu')" },
    ...over,
  };
}

function one(profile: ExplainProfile): ExplainResponse {
  return { outcome: "per_profile", profiles: [profile] };
}

async function renderOpen(result: ExplainResponse) {
  render(
    <MemoryRouter>
      <ExplainResultView result={result} />
    </MemoryRouter>,
  );
  await userEvent.click(screen.getByText("Which policies decided this"));
}

describe("ExplainResultView — an OPA profile", () => {
  test("names the mechanism on the profile header", async () => {
    await renderOpen(one(opaProfile()));
    expect(screen.getByTestId("explain-mechanism")).toHaveTextContent("OPA");
  });

  test("names the engines not yet re-applied to the profile's mechanism", async () => {
    await renderOpen(one(opaProfile({ engines_not_reapplied: ["trino-a", "trino-b"] })));
    expect(screen.getByTestId("explain-not-reapplied")).toHaveTextContent(
      "Not re-applied yet: trino-a, trino-b still run the previous plugin, so they may answer " +
        "differently from this OPA explanation until re-applied (send the switch again).",
    );
  });

  test("says nothing about re-applying when every engine is on the mechanism", async () => {
    await renderOpen(one(opaProfile()));
    expect(screen.queryByTestId("explain-not-reapplied")).toBeNull();
  });

  test("a Ranger profile's header says Ranger", async () => {
    await renderOpen(one(opaProfile({ mechanism: "ranger", opa: undefined })));
    expect(screen.getByTestId("explain-mechanism")).toHaveTextContent("Ranger");
  });

  test("an allow is explained by ir_reason, never as a Ranger policy outside the model", async () => {
    await renderOpen(one(opaProfile()));
    const access = screen.getByTestId("explain-access");
    expect(within(access).getByTestId("decision-badge")).toHaveTextContent("Allowed");
    expect(within(access).getByTestId("explain-ir-reason")).toHaveTextContent(
      "Decided by egret’s evaluator: grant 6f1c",
    );
    expect(access).not.toHaveTextContent(/Outside the permission model/);
    expect(access).not.toHaveTextContent(/Ranger policy/);
    expect(access).not.toHaveTextContent(/policy set this engine downloads/);
    expect(screen.queryByTestId("raw-policy")).not.toBeInTheDocument();
  });

  test("an explicit deny shows its reason, and no empty Ranger policy", async () => {
    await renderOpen(
      one(
        opaProfile({
          accesses: [
            {
              access_type: "select",
              decision: { kind: "denied", policy_id: null, policy_name: "", reason: "deny_policy_item" },
              attribution: null,
              ir_reason: "withhold pii refuses Read on lake.s.t.ssn",
            },
          ],
          opa: undefined,
        }),
      ),
    );
    const access = screen.getByTestId("explain-access");
    expect(within(access).getByTestId("decision-badge")).toHaveTextContent("Refused by a deny item");
    expect(within(access).getByTestId("explain-ir-reason")).toHaveTextContent(
      "withhold pii refuses Read on lake.s.t.ssn",
    );
    expect(access).not.toHaveTextContent(/Ranger policy/);
    expect(access).not.toHaveTextContent(/Outside the permission model/);
  });

  test("a refused profile shows the refusal class as its reason", async () => {
    await renderOpen(
      one(
        opaProfile({
          accesses: [
            {
              access_type: "select",
              decision: { kind: "denied", policy_id: null, policy_name: "", reason: "no_policy_matched" },
              attribution: null,
              ir_reason: "no OPA snapshot: build failed (StoredStateRefused)",
            },
          ],
          opa: undefined,
        }),
      ),
    );
    expect(screen.getByTestId("explain-ir-reason")).toHaveTextContent(
      "no OPA snapshot: build failed (StoredStateRefused)",
    );
  });

  test("renders the column mask with its type and the row filter as code", async () => {
    await renderOpen(one(opaProfile()));
    const mask = screen.getByTestId("explain-opa-mask");
    expect(mask).toHaveTextContent(`Column mask: MASK_HASH, applied as ${MASK}`);
    expect(within(mask).getByText(MASK).tagName).toBe("CODE");
    const filter = screen.getByTestId("explain-opa-row-filter");
    expect(filter).toHaveTextContent("Row filter: (region = 'eu')");
    expect(within(filter).getByText("(region = 'eu')").tagName).toBe("CODE");
  });

  test("says so when no mask or filter applies", async () => {
    await renderOpen(one(opaProfile({ opa: { column_mask: null, column_mask_type: null, row_filter: null } })));
    expect(screen.getByTestId("explain-opa-mask")).toHaveTextContent("No column mask applies.");
    expect(screen.getByTestId("explain-opa-row-filter")).toHaveTextContent("No row filter applies.");
  });

  test("shows no Ranger download tag block", async () => {
    await renderOpen(one(opaProfile()));
    expect(screen.queryByTestId("profile-tags-no-policies")).not.toBeInTheDocument();
  });
});

describe("MembersExplainView — an OPA-masked member", () => {
  test("is counted as masked", () => {
    const response: ExplainPrincipalResponse = {
      membersTotal: 2,
      limit: 200,
      truncated: false,
      evaluated: [
        { userId: "u-ana", label: "Ana", username: "ana", result: one(opaProfile()) },
        {
          userId: "u-bob",
          label: "Bob",
          username: "bob",
          result: one(opaProfile({ opa: { column_mask: null, column_mask_type: null, row_filter: null } })),
        },
      ],
      notEvaluated: [],
    };
    render(
      <MemoryRouter>
        <MembersExplainView response={response} kind="group" />
      </MemoryRouter>,
    );
    const counts = Object.fromEntries(
      screen.getAllByRole("term").map((dt) => [dt.textContent, dt.nextElementSibling?.textContent]),
    );
    expect(counts["Members whose read is masked on at least one"]).toBe("1");
  });
});
