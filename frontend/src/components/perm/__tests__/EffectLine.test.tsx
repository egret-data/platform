import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, test } from "vitest";

import type { ExplainEffectLine } from "@/api/generated/model";
import { EffectLine } from "@/components/perm/EffectLine";

/**
 * The server builds each line as parts plus their joined text (spec C1); the
 * client renders the parts. `text` is written out here by hand, as the server
 * would send it, so a part rendered twice, dropped or reworded fails.
 */
const EXEMPT: ExplainEffectLine = {
  about: "column",
  outcome: "real_value",
  text: "member-sso reads email as the real value (exempt from rule Hash personal data via group data-team; cleared for PII)",
  parts: [
    { kind: "subject", text: "member-sso" },
    { kind: "text", text: " reads " },
    { kind: "code", text: "email" },
    { kind: "text", text: " as " },
    { kind: "strong", text: "the real value" },
    { kind: "text", text: " (exempt from rule " },
    { kind: "rule", text: "Hash personal data", rule_id: "7f1c0e52-0000-4000-8000-000000000001" },
    { kind: "text", text: " via group " },
    { kind: "principal", text: "data-team", principal_kind: "group", ref: "data-team" },
    { kind: "text", text: "; cleared for " },
    { kind: "clearance", text: "PII", tag_id: "tag-pii" },
    { kind: "text", text: ")" },
  ],
};

function renderLine(line: ExplainEffectLine) {
  render(
    <MemoryRouter>
      <EffectLine line={line} />
    </MemoryRouter>,
  );
  return screen.getByTestId("effect-line");
}

describe("EffectLine — a server-built sentence, rendered from its parts", () => {
  test("its text is exactly the server's text", () => {
    const line = renderLine(EXEMPT);
    expect(line.textContent).toBe(EXEMPT.text);
    expect(line).toHaveAttribute("data-outcome", "real_value");
  });

  test("the outcome is bold, the column is mono", () => {
    const line = renderLine(EXEMPT);
    const strong = within(line).getByText("the real value");
    expect(strong.tagName).toBe("STRONG");
    expect(within(line).getByText("email").tagName).toBe("CODE");
    // Only the outcome is bold.
    expect(line.querySelectorAll("strong")).toHaveLength(1);
  });

  test("the rule, the clearance and the principal are links to their pages, named as in the sentence", () => {
    const line = renderLine(EXEMPT);
    expect(within(line).getByRole("link", { name: "Hash personal data" })).toHaveAttribute(
      "href",
      "/governance/rules/7f1c0e52-0000-4000-8000-000000000001",
    );
    expect(within(line).getByRole("link", { name: "PII" })).toHaveAttribute(
      "href",
      "/governance/classifications/tag-pii?tab=clearance",
    );
    expect(within(line).getByRole("link", { name: "data-team" })).toHaveAttribute(
      "href",
      "/governance/people/groups/data-team",
    );
    expect(within(line).getAllByRole("link")).toHaveLength(3);
  });

  test("a user named directly links to their page by username", () => {
    const line = renderLine({
      about: "rows",
      outcome: "all_rows",
      text: "carol sees every row (exempt from rule US rows only via user carol)",
      parts: [
        { kind: "subject", text: "carol" },
        { kind: "text", text: " sees " },
        { kind: "strong", text: "every row" },
        { kind: "text", text: " (exempt from rule " },
        { kind: "rule", text: "US rows only", rule_id: "r-2" },
        { kind: "text", text: " via user " },
        { kind: "principal", text: "carol", principal_kind: "user", ref: "carol" },
        { kind: "text", text: ")" },
      ],
    });
    expect(within(line).getByRole("link", { name: "carol" })).toHaveAttribute(
      "href",
      "/governance/people/users/by-name/carol",
    );
  });

  test("a manual policy is named but not linked; a filter expression is mono", () => {
    const line = renderLine({
      about: "rows",
      outcome: "filtered",
      text: "ana sees rows where region = 'us' (manual policy us-only)",
      parts: [
        { kind: "subject", text: "ana" },
        { kind: "text", text: " sees " },
        { kind: "strong", text: "rows where" },
        { kind: "text", text: " " },
        { kind: "code", text: "region = 'us'" },
        { kind: "text", text: " (manual policy " },
        { kind: "policy", text: "us-only", policy_id: 4 },
        { kind: "text", text: ")" },
      ],
    });
    expect(line.textContent).toBe("ana sees rows where region = 'us' (manual policy us-only)");
    expect(within(line).getByText("us-only").tagName).toBe("EM");
    expect(within(line).getByText("region = 'us'").tagName).toBe("CODE");
    expect(within(line).queryByRole("link")).not.toBeInTheDocument();
  });

  test("the Not enforced line links the catalog to its Data page", () => {
    const line = renderLine({
      about: "access",
      outcome: "not_enforced",
      text: "Not enforced: demo_warehouse has no engine.",
      parts: [
        { kind: "text", text: "Not enforced: " },
        { kind: "catalog", text: "demo_warehouse", catalog_id: "c-demo" },
        { kind: "text", text: " has no engine." },
      ],
    });
    expect(line.textContent).toBe("Not enforced: demo_warehouse has no engine.");
    expect(within(line).getByRole("link", { name: "demo_warehouse" })).toHaveAttribute(
      "href",
      "/governance/data/demo_warehouse",
    );
  });
});
