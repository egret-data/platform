import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, within } from "@testing-library/react";
import {
  E_SR,
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

/*
 * Ported from `routes/governance/__tests__/ProfileDetailPage.test.tsx`
 * ("stored policies Ranger reads differently (SP4d-4 B1)"): the callout
 * (`PolicyWarningsCallout`, unchanged in #4a, spec §6.2) now sits above the
 * Policies tab, and its editor links open the Advanced editor of the page it
 * is on.
 */

const T = { timeout: 3000 };
const SELECT_SENTENCE = "Stops at table but allows select: it grants no column read, only table-level checks.";
const ROWS = [policy(42, "operator", { name: "allow-analysts" }), policy(43, "operator", { name: "gap-policy", policy_type: "rowfilter" })];

function answers(
  warnings: Answer,
  { type = "trino", list = ok(ROWS) as Answer, engine = E_TR } = {},
): Record<string, Answer> {
  const name = type === "trino" ? "lake-tr" : "lake-sr";
  return {
    [`/perm/engines/${engine}/grants`]: ok(
      grantsResponse({
        engine: { id: engine, name, type, origin: "provisioned", status: "running" },
        profile: { serviceId: 2, name },
        reach: { engines: [{ id: engine, name, origin: "provisioned" }], sharedProfile: false },
      }),
    ),
    "/policies/services": ok([
      profileDto(name, {
        id: 2,
        type,
        engines: [{ id: engine, name, status: "running", origin: "provisioned", type }],
      }),
    ]),
    [`/policies?service=${name}`]: list,
    "/perm/overlaps": ok([]),
    "/perm/policies/services/2/warnings": warnings,
    [`/governance/engines/${engine}/plugin`]: ok(pluginResponse()),
  };
}

const open = (engine = E_TR) => openArea(`/governance/engines/${engine}?tab=policies`, READER);
const calloutOf = async (title: string) =>
  (await screen.findByText(title, {}, T)).closest('[role="alert"]') as HTMLElement;

describe("the policy warnings callout on the Policies tab (SP4d-4 B1)", () => {
  it("shows no callout for a profile with warningCount 0", async () => {
    serve(answers(ok({ warningCount: 0, policies: [] })));
    open();
    await screen.findByText("allow-analysts", {}, T);
    expect(screen.queryByText(/stored polic(y|ies)$/)).not.toBeInTheDocument();
  });

  it("asks for the warnings of THIS profile's id", async () => {
    const served = serve(answers(ok({ warningCount: 0, policies: [] })));
    open();
    await screen.findByText("allow-analysts", {}, T);
    expect(served.asked).toContain("GET /perm/policies/services/2/warnings");
  });

  it("shows one callout with the count, each policy's sentence, a link to its editor, the measured reason and the remedy", async () => {
    serve(
      answers(
        ok({
          warningCount: 2,
          policies: [
            { id: 42, name: "allow-analysts", warnings: [{ kind: "select_stops_above_column", level: "table", sentence: SELECT_SENTENCE }] },
            { id: 43, name: "gap-policy", warnings: [{ kind: "level_gap", missing: "schema", sentence: "Names table under an absent schema." }] },
          ],
        }),
      ),
    );
    open();
    const callout = await calloutOf("2 warnings on 2 stored policies");
    expect(callout).not.toBeNull();
    expect(within(callout).getByText(SELECT_SENTENCE)).toBeInTheDocument();
    expect(within(callout).getByText("Names table under an absent schema.")).toBeInTheDocument();
    // The link carries the policy's own type (found from the list), and opens the Advanced editor of this page.
    expect(within(callout).getByRole("link", { name: "allow-analysts" })).toHaveAttribute(
      "href",
      `/governance/engines/${E_TR}/advanced/policies/access/42`,
    );
    expect(within(callout).getByRole("link", { name: "gap-policy" })).toHaveAttribute(
      "href",
      `/governance/engines/${E_TR}/advanced/policies/rowfilter/43`,
    );
    expect(callout).toHaveTextContent("does not grant column SELECT on Trino 470 or StarRocks 4.1.4");
    expect(callout).toHaveTextContent("set the column level");
    expect(callout).toHaveTextContent("Nothing was changed");
  });

  it.each([
    ["trino", E_TR, "Trino still allows table-level checks such as count(*)", /StarRocks denies/],
    ["starrocks", E_SR, "StarRocks denies even count(*)", /Trino still allows/],
  ])("words the count(*) consequence for a %s profile", async (type, engine, said, unsaid) => {
    serve(
      answers(
        ok({
          warningCount: 1,
          policies: [
            { id: 42, name: "allow-analysts", warnings: [{ kind: "select_stops_above_column", level: "table", sentence: SELECT_SENTENCE }] },
          ],
        }),
        { type, engine },
      ),
    );
    open(engine);
    const callout = await calloutOf("1 warning on 1 stored policy");
    expect(callout).toHaveTextContent(said);
    expect(callout).not.toHaveTextContent(unsaid);
  });

  it("explains only the kinds that are present", async () => {
    serve(
      answers(
        ok({
          warningCount: 1,
          policies: [{ id: 43, name: "gap-policy", warnings: [{ kind: "level_gap", missing: "schema", sentence: "gap." }] }],
        }),
      ),
    );
    open();
    const callout = await calloutOf("1 warning on 1 stored policy");
    expect(callout).toHaveTextContent("A level is named below one that is not set");
    expect(callout).not.toHaveTextContent("column SELECT");
  });

  it("shows a policy's name as plain text, not a guessed link, while its kind is unknown", async () => {
    serve(
      answers(
        ok({
          warningCount: 1,
          policies: [{ id: 42, name: "allow-analysts", warnings: [{ kind: "level_gap", missing: "schema", sentence: "gap." }] }],
        }),
        { list: fail(500) },
      ),
    );
    open();
    const callout = await calloutOf("1 warning on 1 stored policy");
    expect(within(callout).getByText("allow-analysts")).toBeInTheDocument();
    expect(within(callout).queryByRole("link", { name: "allow-analysts" })).toBeNull();
  });
});
