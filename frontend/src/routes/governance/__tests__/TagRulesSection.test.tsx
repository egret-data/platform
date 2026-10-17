import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * A tag's "Mask and filter rules" (DG redesign #2, P9): every egret rule a
 * tag-selected mask or row filter reaches this tag through. Engines match a
 * tag rule by the tag's NAME (case-sensitively, `*`/`?` glob), so a rule
 * written on another tag of the same name is listed too, and says so.
 */
vi.mock("@/api/generated/perm/perm");
vi.mock("@/api/generated/control-plane-governance/control-plane-governance");

import * as permApi from "@/api/generated/perm/perm";
import * as govApi from "@/api/generated/control-plane-governance/control-plane-governance";
import type { RuleView } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import TagRulesSection from "@/routes/governance/TagRulesSection";

const TAG = { key: "pii_eu", name: "PII" };

function tagRule(over: Partial<RuleView> & { key?: string }): RuleView {
  const { key = "pii_eu", ...rest } = over;
  return {
    id: "r1",
    name: "mask ssn",
    kind: { type: "mask", mask_type: "MASK_HASH" },
    selector: { type: "tag", key },
    exempt: { include: [], exclude: [] },
    tag_name: "PII",
    ...rest,
  };
}

function serveRules(rows: RuleView[]) {
  vi.mocked(permApi.usePermListRules).mockReturnValue({
    data: { status: 200, data: rows },
    isLoading: false,
    isError: false,
    error: null,
    refetch: vi.fn(),
  } as unknown as ReturnType<typeof permApi.usePermListRules>);
}

function renderSection() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <TagRulesSection tag={TAG} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const region = () => screen.getByRole("region", { name: "Mask and filter rules" });

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(govApi.useControlPlaneGovernanceListCatalogs).mockReturnValue({
    data: { status: 200, data: [] },
  } as unknown as ReturnType<typeof govApi.useControlPlaneGovernanceListCatalogs>);
});

describe("TagRulesSection", () => {
  test("lists each rule whose tag name matches, linked to its page; a rule on another tag of the same name says so", () => {
    serveRules([
      tagRule({ id: "r-own", name: "mask ssn", exempt: { include: [{ kind: "role", ref: "auditor" }], exclude: [] } }),
      tagRule({ id: "r-us", name: "null email", key: "pii_us", kind: { type: "row_filter", filter_expr: "region = 'eu'" } }),
      tagRule({ id: "r-glob", name: "hash all P", key: "p", tag_name: "P*" }),
      tagRule({ id: "r-lower", name: "lowercase", key: "pii_low", tag_name: "pii" }),
      tagRule({ id: "r-gone", name: "orphan", tag_name: null }),
      {
        id: "r-path",
        name: "path rule",
        kind: { type: "mask", mask_type: "MASK_NULL" },
        selector: { type: "path", catalog_id: "c", schema: "s" },
        exempt: { include: [], exclude: [] },
        tag_name: null,
      },
    ]);
    renderSection();
    const rows = within(region()).getAllByRole("row").slice(1);
    expect(rows.map((r) => within(r).getAllByRole("cell")[0].textContent)).toEqual(["hash all P", "mask ssn", "null email"]);

    expect(within(region()).getByRole("link", { name: "mask ssn" })).toHaveAttribute("href", "/governance/rules/r-own");
    expect(within(region()).getByRole("link", { name: "null email" })).toHaveAttribute("href", "/governance/rules/r-us");
    expect(rows[1]).toHaveTextContent("This tag");
    expect(rows[1]).toHaveTextContent("mask · MASK_HASH");
    expect(within(rows[1]).getByRole("link", { name: "auditor" })).toHaveAttribute("href", "/governance/people/roles/auditor");
    expect(rows[2]).toHaveTextContent("Written on tag pii_us, which has this tag's name too");
    expect(rows[2]).toHaveTextContent("row filter · region = 'eu'");
    expect(rows[0]).toHaveTextContent("Written on tag p, whose name “P*” matches this tag's name");
    expect(region()).not.toHaveTextContent("lowercase");
    expect(region()).not.toHaveTextContent("orphan");
    expect(region()).not.toHaveTextContent("path rule");
  });

  test("says when no rule reaches the tag", () => {
    serveRules([tagRule({ tag_name: "Finance", key: "fin" })]);
    renderSection();
    expect(within(region()).getByText("No egret rule selects this tag.")).toBeInTheDocument();
  });

  test("a failed read shows its error with a retry, never the empty state", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(500, "boom"),
      refetch: vi.fn(),
    } as unknown as ReturnType<typeof permApi.usePermListRules>);
    renderSection();
    expect(within(region()).getByText("The rules could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(region()).queryByText("No egret rule selects this tag.")).not.toBeInTheDocument();
  });
});
