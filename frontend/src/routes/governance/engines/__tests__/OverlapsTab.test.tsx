import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, within } from "@testing-library/react";
import type { StoredOverlap } from "@/api/generated/model";
import {
  E_TR,
  fail,
  grantsResponse,
  ok,
  openArea,
  pluginResponse,
  policy,
  profileDto,
  READER,
  serve,
  type Answer,
} from "./engineHarness";

afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };

/*
 * Ported from `routes/governance/__tests__/OverlapReport.test.tsx`: the cases
 * that describe one profile's pairs. The landing-page cases (every profile, a
 * profile-name link per pair, a tag service no profile names) describe the
 * cross-profile report the index no longer shows (spec §6.3).
 */

function overlap(over: Partial<StoredOverlap> = {}): StoredOverlap {
  return {
    serviceId: 2,
    serviceName: "lake-tr",
    kind: "mask",
    coordinate: "catalog=[wh] column=[email] schema=[sales] table=[orders]",
    rules: [
      { id: "r-a", policyId: 11, name: "mask emails" },
      { id: "r-b", policyId: 12, name: "null orders email" },
    ],
    servedToday: { id: "r-a", policyId: 11, name: "mask emails" },
    sentence: "two mask policies would be served at policyPriority 0",
    howToFix: "Delete one of the two. PUT /api/control-plane/policies/{id}/enabled (use its policyId).",
    ...over,
  };
}

const HAND = overlap({
  kind: "row_filter",
  coordinate: "catalog=[wh] schema=[sales] table=[orders]",
  rules: [
    { id: null, policyId: 31, name: "hand filter" },
    { id: "r-c", policyId: 32, name: "region filter" },
  ],
  servedToday: null,
});

function answers(overlaps: Answer, over: Record<string, Answer> = {}): Record<string, Answer> {
  return {
    [`/perm/engines/${E_TR}/grants`]: ok(grantsResponse()),
    "/policies/services": ok([
      profileDto("lake-tr", {
        id: 2,
        engines: [{ id: E_TR, name: "lake-trino", status: "running", origin: "provisioned", type: "trino" }],
      }),
    ]),
    "/policies?service=lake-tr": ok([policy(1, "compiler")]),
    [`/governance/engines/${E_TR}/plugin`]: ok(pluginResponse()),
    "/perm/overlaps": overlaps,
    ...over,
  };
}

const open = () => openArea(`/governance/engines/${E_TR}?tab=overlaps`, READER);
const pairs = () => screen.findAllByRole("listitem", { name: /on catalog=/ }, T);

describe("the Overlaps tab (spec §6.3)", () => {
  it("lists only the pairs of this profile, each with both rules, the shared object and why", async () => {
    serve(
      answers(
        ok([
          overlap(),
          HAND,
          overlap({ serviceId: 9, serviceName: "other-profile", rules: [{ id: "r-x", policyId: 1, name: "elsewhere" }, { id: "r-y", policyId: 2, name: "elsewhere too" }] }),
        ]),
      ),
    );
    open();
    const items = await pairs();
    expect(items).toHaveLength(2);
    expect(screen.queryByText(/elsewhere/)).not.toBeInTheDocument();

    const first = items.find((i) => i.textContent?.includes("mask emails"))!;
    expect(within(first).getByText("Mask")).toBeInTheDocument();
    expect(within(first).getByText("catalog=[wh] column=[email] schema=[sales] table=[orders]")).toBeInTheDocument();
    expect(within(first).getByRole("link", { name: "rule mask emails" })).toHaveAttribute("href", "/governance/rules/r-a");
    expect(within(first).getByRole("link", { name: "rule null orders email" })).toHaveAttribute(
      "href",
      "/governance/rules/r-b",
    );
    expect(within(first).getByText(/today the engine serves mask emails\./)).toBeInTheDocument();
    expect(within(first).getByText("two mask policies would be served at policyPriority 0")).toBeInTheDocument();
  });

  it("a hand-written member opens the Advanced editor, and Switch off links there; no API route is shown", async () => {
    serve(answers(ok([HAND])));
    open();
    const [item] = await pairs();
    expect(within(item).getByText("Row filter")).toBeInTheDocument();
    expect(within(item).getByRole("link", { name: "policy hand filter" })).toHaveAttribute(
      "href",
      `/governance/engines/${E_TR}/advanced/policies/rowfilter/31`,
    );
    expect(within(item).getByRole("link", { name: "Switch off policy hand filter" })).toHaveAttribute(
      "href",
      `/governance/engines/${E_TR}/advanced/policies/rowfilter/31`,
    );
    expect(within(item).queryByRole("link", { name: /Switch off policy region filter/ })).toBeNull();
    expect(item).not.toHaveTextContent("PUT");
    expect(item).not.toHaveTextContent("/api/control-plane");
    expect(item).not.toHaveTextContent("policyId");
  });

  it("the same pair reported twice is one pair", async () => {
    serve(answers(ok([overlap(), overlap()])));
    open();
    expect(await pairs()).toHaveLength(1);
  });

  it("the tab trigger's count equals the pairs shown", async () => {
    serve(answers(ok([overlap(), HAND, overlap({ serviceName: "other-profile" })])));
    open();
    expect(await pairs()).toHaveLength(2);
    expect(screen.getByRole("tab", { name: /Overlaps/ })).toHaveTextContent("Overlaps2");
  });

  it("an empty report says so", async () => {
    serve(answers(ok([overlap({ serviceName: "other-profile" })])));
    open();
    expect(await screen.findByText("No overlapping rules on this profile.", {}, T)).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /Overlaps/ })).toHaveTextContent("Overlaps0");
  });

  it("a failing read shows the error and Retry", async () => {
    serve(answers(fail(500)));
    open();
    expect(await screen.findByText(/The overlaps could not be loaded/, {}, T)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("an unbound profile's page lists its pairs and counts them on the tab", async () => {
    serve({
      "/policies/services": ok([profileDto("loose", { id: 5, origin: "manual" })]),
      "/policies?service=loose": ok([policy(1, "operator", { service: "loose" })]),
      "/perm/policies/services/5/warnings": ok({ warningCount: 0, policies: [] }),
      "/perm/overlaps": ok([overlap({ serviceId: 5, serviceName: "loose" }), overlap()]),
    });
    openArea("/governance/engines/profiles/loose?tab=overlaps", READER);
    const items = await pairs();
    expect(items).toHaveLength(1);
    expect(screen.getByRole("tab", { name: /Overlaps/ })).toHaveTextContent("Overlaps1");
  });
});
