import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * The every-profile overlap report (SP4d-4), rendered directly. These tests
 * lived in `MaskFilterRules.test.tsx`, which rendered the report on that page;
 * the page is gone (DG redesign #2, P8) and the report is on the Policies
 * landing and on each profile's page. A compiled member now links to its
 * rule's own page, `/governance/rules/:id`.
 */
vi.mock("@/api/generated/perm/perm");
// The profiles that resolve an overlap's service name to an editor route.
let profiles: Array<{ id: number; name: string; type: string; tag_service?: string | null }> = [];
vi.mock("@/api/policies", async (orig) => ({
  ...(await orig<typeof import("@/api/policies")>()),
  listProfiles: () => Promise.resolve(profiles),
}));

import * as permApi from "@/api/generated/perm/perm";
import type { StoredOverlap } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { OverlapReport } from "@/routes/governance/OverlapReport";

function overlap(over: Partial<StoredOverlap> = {}): StoredOverlap {
  return {
    serviceId: 7,
    serviceName: "trino-eu",
    kind: "mask",
    coordinate: "catalog=[wh] column=[email] schema=[sales] table=[orders]",
    rules: [
      { id: "r-a", policyId: 11, name: "mask emails" },
      { id: "r-b", policyId: 12, name: "null orders email" },
    ],
    servedToday: { id: "r-a", policyId: 11, name: "mask emails" },
    sentence: 'two mask policies would be served at policyPriority 0 ...',
    howToFix: "Delete one of the two, or narrow one so they no longer select a common resource.",
    ...over,
  };
}

function overlapsResult(rows: StoredOverlap[]) {
  return { data: { status: 200, data: rows }, isLoading: false, isError: false, error: null } as unknown as ReturnType<
    typeof permApi.usePermListOverlaps
  >;
}

function renderReport() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MemoryRouter>
      <QueryClientProvider client={qc}>
        <OverlapReport />
      </QueryClientProvider>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  profiles = [
    { id: 7, name: "trino-eu", type: "trino" },
    { id: 8, name: "trino-us", type: "trino" },
  ];
  vi.mocked(permApi.usePermListOverlaps).mockReturnValue(overlapsResult([]));
});

describe("OverlapReport", () => {
  test("a banner names both rules of each pair and links to each rule", async () => {
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(
      overlapsResult([
        overlap(),
        overlap({
          serviceId: 8,
          serviceName: "trino-us",
          rules: [
            { id: null, policyId: 31, name: "hand email" },
            { id: "r-c", policyId: 32, name: "mask emails us" },
          ],
          servedToday: null,
        }),
      ]),
    );
    renderReport();

    const banner = screen.getByRole("alert");
    expect(within(banner).getByText("2 pairs of rules overlap")).toBeInTheDocument();
    await within(banner).findByRole("link", { name: "trino-eu" });
    expect(within(banner).getByRole("link", { name: "rule mask emails" })).toHaveAttribute(
      "href",
      "/governance/rules/r-a",
    );
    expect(within(banner).getByRole("link", { name: "rule null orders email" })).toHaveAttribute(
      "href",
      "/governance/rules/r-b",
    );
    // A hand-written member opens its policy in that profile's editor.
    expect(within(banner).getByRole("link", { name: "policy hand email" })).toHaveAttribute(
      "href",
      "/governance/policies/trino-us/masking/31",
    );
    expect(within(banner).getByRole("link", { name: "trino-eu" })).toHaveAttribute(
      "href",
      "/governance/policies/trino-eu",
    );
    expect(within(banner).getByText(/today the engine serves mask emails\./)).toBeInTheDocument();
  });

  test("one pair reads in the singular", () => {
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(overlapsResult([overlap()]));
    renderReport();
    expect(screen.getByText("1 pair of rules overlaps")).toBeInTheDocument();
  });

  test("an empty report shows no banner", () => {
    const { container } = renderReport();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(container).toBeEmptyDOMElement();
  });

  test("a report that fails to load says so rather than showing nothing", () => {
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(500, "boom"),
    } as unknown as ReturnType<typeof permApi.usePermListOverlaps>);
    renderReport();
    expect(screen.getByRole("alert")).toHaveTextContent("Could not check for overlapping rules");
  });
});

describe("OverlapReport reads as an operator action (SP4d-4 Task 12)", () => {
  const handPair = (over: Partial<StoredOverlap> = {}) =>
    overlap({
      rules: [
        { id: null, policyId: 31, name: "hand email" },
        { id: "r-c", policyId: 32, name: "mask emails us" },
      ],
      servedToday: null,
      howToFix:
        "Delete one of the two. A hand-written member can also be switched off with PUT /api/control-plane/policies/{id}/enabled (use its policyId).",
      ...over,
    });

  test("never shows the API route; offers a link to switch the hand-written policy off", async () => {
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(overlapsResult([handPair()]));
    renderReport();
    const banner = screen.getByRole("alert");
    const link = await within(banner).findByRole("link", { name: "Switch off policy hand email" });
    expect(link).toHaveAttribute("href", "/governance/policies/trino-eu/masking/31");
    expect(banner).toHaveTextContent("with the Enabled switch in its editor");
    expect(banner).not.toHaveTextContent("PUT");
    expect(banner).not.toHaveTextContent("/api/control-plane");
    expect(banner).not.toHaveTextContent("policyId");
    // A compiled rule has no such switch: no "switch off" link for it.
    expect(within(banner).queryByRole("link", { name: /Switch off policy mask emails us/ })).toBeNull();
  });

  test("a pair that exists only in a tag service no profile names is plain text, not a dead link", async () => {
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(
      overlapsResult([handPair({ serviceName: "orphan_tag", serviceId: 99 })]),
    );
    renderReport();
    const banner = screen.getByRole("alert");
    // Let the profiles query settle, then check nothing links into the tag service.
    await within(banner).findByRole("link", { name: "rule mask emails us" });
    expect(within(banner).queryByRole("link", { name: "policy hand email" })).toBeNull();
    expect(within(banner).queryByRole("link", { name: "orphan_tag" })).toBeNull();
    expect(within(banner).getByText("policy hand email")).toBeInTheDocument();
    expect(within(banner).queryByRole("link", { name: /Switch off policy/ })).toBeNull();
    for (const a of within(banner).queryAllByRole("link")) {
      expect(a.getAttribute("href")).not.toContain("orphan_tag");
    }
  });

  test("a tag service a profile names opens its policies through that profile", async () => {
    profiles = [
      { id: 7, name: "trino-eu", type: "trino", tag_service: "eu_tag" },
      { id: 70, name: "eu_tag", type: "tag" },
    ];
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(
      overlapsResult([handPair({ serviceName: "eu_tag", serviceId: 70 })]),
    );
    renderReport();
    const link = await screen.findByRole("link", { name: "policy hand email" });
    expect(link).toHaveAttribute("href", "/governance/policies/trino-eu/masking/31");
  });

  test("a tag service shared by two profiles is ONE pair, naming both profiles", async () => {
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(
      overlapsResult([
        handPair({ serviceName: "trino-eu", serviceId: 7 }),
        handPair({ serviceName: "trino-us", serviceId: 8 }),
      ]),
    );
    renderReport();
    const banner = screen.getByRole("alert");
    expect(within(banner).getByText("1 pair of rules overlaps")).toBeInTheDocument();
    expect(await within(banner).findByRole("link", { name: "trino-eu" })).toBeInTheDocument();
    expect(within(banner).getByRole("link", { name: "trino-us" })).toBeInTheDocument();
    // A different pair is still counted separately.
    expect(within(banner).getAllByText("policy hand email")).toHaveLength(1);
  });

  test("two different pairs are two pairs", async () => {
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(
      overlapsResult([
        handPair(),
        handPair({
          serviceName: "trino-us",
          rules: [
            { id: null, policyId: 41, name: "other hand" },
            { id: "r-d", policyId: 42, name: "other rule" },
          ],
        }),
      ]),
    );
    renderReport();
    expect(screen.getByText("2 pairs of rules overlap")).toBeInTheDocument();
    await screen.findByRole("link", { name: "trino-eu" });
  });

  test("a refused read shows nothing: the page around it says what the reader lacks", () => {
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(403, "forbidden"),
    } as unknown as ReturnType<typeof permApi.usePermListOverlaps>);
    const { container } = renderReport();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(container).toBeEmptyDOMElement();
  });
});
