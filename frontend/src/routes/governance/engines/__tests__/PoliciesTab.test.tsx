import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  E_TR,
  fail,
  grantsResponse,
  me,
  ok,
  OPERATOR,
  openArea,
  policy,
  pluginResponse,
  profileDto,
  READER,
  serve,
  where,
  type Answer,
} from "./engineHarness";

afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const G = "3f1c9a2e-5b7d-4c11-9e0a-2b6d8f4a1c35";
const R = "4a2d0b3f-6c8e-4d22-8f1b-3c7e9a5b2d46";
const TAG = "a0b1c2d3-e4f5-4a6b-8c7d-9e0f1a2b3c4d";
const EG = "5b3e1c4a-7d9f-4e33-9a2c-4d8f0b6c3e57";

const WRITER = me(["policy:read", "policy:update", "policy:delete", "tag:read"]);

/** Not pre-sorted: the hand-written row first, a tag-service row in the middle (FE-TEST-1). */
const MAIN = [
  policy(10, "operator", { name: "zeta hand-written", policy_type: "masking" }),
  policy(11, "compiler", { name: `perm-grant-${G}` }),
  policy(12, "infra", { name: "egret-admin - catalog, schema, table, column" }),
  policy(13, "compiler", { name: `perm-rule-${R}`, policy_type: "rowfilter" }),
  policy(14, "compiler", { name: `perm-engine-${EG}` }),
  policy(15, "compiler", { name: "perm-clearance-mv" }),
  policy(16, "compiler", { name: "perm-something-odd" }),
];
const TAGROWS = [
  policy(20, "compiler", { name: `perm-clearance-${TAG}`, service: "lake-tr-tag" }),
  policy(21, "operator", { name: "alpha by tag", service: "lake-tr-tag", policy_type: "access" }),
];

function answers(over: Record<string, Answer> = {}): Record<string, Answer> {
  return {
    [`/perm/engines/${E_TR}/grants`]: ok(grantsResponse()),
    "/policies/services": ok([
      profileDto("lake-tr", {
        id: 2,
        tag_service: "lake-tr-tag",
        engines: [{ id: E_TR, name: "lake-trino", status: "running", origin: "provisioned", type: "trino" }],
      }),
      profileDto("lake-tr-tag", { id: 1, type: "tag" }),
    ]),
    "/policies?service=lake-tr": ok(MAIN),
    "/policies?service=lake-tr-tag": ok(TAGROWS),
    "/perm/policies/services/2/warnings": ok({ warningCount: 0, policies: [] }),
    [`/governance/engines/${E_TR}/plugin`]: ok(pluginResponse()),
    ...over,
  };
}

const open = (query = "", as = OPERATOR) => openArea(`/governance/engines/${E_TR}?tab=policies${query}`, as);
const table = () => screen.findByRole("table", {}, T);
const rowOf = (name: string) => screen.getByRole("row", { name: new RegExp(name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")) });

describe("the Policies tab lists the profile and its tag service (spec §6.2)", () => {
  it("reads both services and lists every row", async () => {
    const { asked } = serve(answers());
    open();
    await table();
    expect(asked).toContain("GET /policies?service=lake-tr");
    expect(asked).toContain("GET /policies?service=lake-tr-tag");
    expect(screen.getByText("zeta hand-written")).toBeInTheDocument();
    expect(screen.getByText("alpha by tag")).toBeInTheDocument();
    expect(screen.getByText(`perm-clearance-${TAG}`)).toBeInTheDocument();
  });

  it("says how many policies each source holds", async () => {
    serve(answers());
    open();
    await table();
    expect(screen.getByText("9 policies: 6 egret-compiled · 1 egret-seeded · 2 hand-written")).toBeInTheDocument();
  });

  it("writes the singular", async () => {
    serve(
      answers({
        "/policies?service=lake-tr": ok([policy(10, "operator")]),
        "/policies?service=lake-tr-tag": ok([]),
      }),
    );
    open();
    await table();
    expect(screen.getByText("1 policy: 0 egret-compiled · 0 egret-seeded · 1 hand-written")).toBeInTheDocument();
  });

  it("offers the Tag type only when the profile has a tag service", async () => {
    serve(answers());
    const first = open();
    await table();
    expect(screen.getByRole("combobox", { name: "Type" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("combobox", { name: "Type" }));
    expect(await screen.findByRole("option", { name: "Tag" })).toBeInTheDocument();
    first.unmount();
    vi.unstubAllGlobals();

    serve(
      answers({
        "/policies/services": ok([
          profileDto("lake-tr", {
            id: 2,
            engines: [{ id: E_TR, name: "lake-trino", status: "running", origin: "provisioned", type: "trino" }],
          }),
        ]),
      }),
    );
    open();
    await table();
    await userEvent.click(screen.getByRole("combobox", { name: "Type" }));
    expect(await screen.findByRole("option", { name: "Masking" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Tag" })).not.toBeInTheDocument();
  });

  it("renders the policy warnings callout", async () => {
    serve(
      answers({
        "/perm/policies/services/2/warnings": ok({
          warningCount: 1,
          policies: [{ id: 10, name: "zeta hand-written", warnings: [{ kind: "level_gap", sentence: "A level is missing." }] }],
        }),
      }),
    );
    open();
    expect(await screen.findByText("1 warning on 1 stored policy", {}, T)).toBeInTheDocument();
  });
});

describe("filters live in the URL", () => {
  it("?source= keeps one source", async () => {
    serve(answers());
    open("&source=operator");
    await table();
    expect(screen.getByText("zeta hand-written")).toBeInTheDocument();
    expect(screen.getByText("alpha by tag")).toBeInTheDocument();
    expect(screen.queryByText(`perm-grant-${G}`)).not.toBeInTheDocument();
  });

  it("?type=masking keeps the profile's masking rows; ?type=tag keeps the tag service's rows", async () => {
    serve(answers());
    const first = open("&type=masking");
    await table();
    expect(screen.getByText("zeta hand-written")).toBeInTheDocument();
    expect(screen.queryByText(`perm-rule-${R}`)).not.toBeInTheDocument();
    expect(screen.queryByText("alpha by tag")).not.toBeInTheDocument();
    first.unmount();

    open("&type=tag");
    await table();
    expect(screen.getByText("alpha by tag")).toBeInTheDocument();
    expect(screen.getByText(`perm-clearance-${TAG}`)).toBeInTheDocument();
    expect(screen.queryByText("zeta hand-written")).not.toBeInTheDocument();
  });

  it("?q= searches names, and typing writes it to the URL", async () => {
    serve(answers());
    open("&q=zeta");
    await table();
    expect(screen.getByText("zeta hand-written")).toBeInTheDocument();
    expect(screen.queryByText("alpha by tag")).not.toBeInTheDocument();
    const box = screen.getByRole("searchbox", { name: "Search policies" });
    await userEvent.clear(box);
    await userEvent.type(box, "alpha");
    await waitFor(() => expect(where()).toContain("q=alpha"));
    expect(screen.getByText("alpha by tag")).toBeInTheDocument();
  });

  it("choosing a source writes ?source=", async () => {
    serve(answers());
    open();
    await table();
    await userEvent.click(screen.getByRole("combobox", { name: "Source" }));
    await userEvent.click(await screen.findByRole("option", { name: "egret-seeded" }));
    await waitFor(() => expect(where()).toContain("source=infra"));
    expect(screen.getByText("egret-admin - catalog, schema, table, column")).toBeInTheDocument();
    expect(screen.queryByText("zeta hand-written")).not.toBeInTheDocument();
  });

  it("a filter that matches nothing says so and clears", async () => {
    serve(answers());
    open("&q=nothing-like-this");
    expect(await screen.findByText("No policy matches these filters.", {}, T)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(where()).not.toContain("q="));
    expect(await table()).toBeInTheDocument();
  });

  it("an empty profile says so", async () => {
    serve(answers({ "/policies?service=lake-tr": ok([]), "/policies?service=lake-tr-tag": ok([]) }));
    open();
    expect(await screen.findByText("This profile has no policy.", {}, T)).toBeInTheDocument();
  });

  it("a failing read shows the error and Retry", async () => {
    serve(answers({ "/policies?service=lake-tr": fail(500) }));
    open();
    expect(await screen.findByText(/The policies could not be loaded/, {}, T)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });
});

describe("controls follow the row's source", () => {
  it("a compiled row is read-only, locked, and opens its source", async () => {
    serve(answers());
    open("", WRITER);
    await table();
    const row = rowOf(`perm-grant-${G}`);
    expect(within(row).getByText("Compiled by egret; change its source")).toBeInTheDocument();
    expect(within(row).queryByRole("switch")).not.toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /Delete/ })).not.toBeInTheDocument();
    expect(within(row).queryByRole("link", { name: /Edit/ })).not.toBeInTheDocument();
    expect(within(row).getByRole("link", { name: /Open source/ })).toHaveAttribute("href", `/governance/grants/${G}`);
    expect(within(rowOf(`perm-rule-${R}`)).getByRole("link", { name: /Open source/ })).toHaveAttribute(
      "href",
      `/governance/rules/${R}`,
    );
    expect(within(rowOf(`perm-clearance-${TAG}`)).getByRole("link", { name: /Open source/ })).toHaveAttribute(
      "href",
      `/governance/classifications/${TAG}?tab=clearance`,
    );
    expect(within(rowOf(`perm-engine-${EG}`)).getByRole("link", { name: /Open source/ })).toHaveAttribute(
      "href",
      `/governance/engines/${E_TR}?tab=engine-permissions&grant=${EG}`,
    );
  });

  it("the MV deny and an unparsable compiled name name their kind and link nowhere", async () => {
    serve(answers());
    open("", WRITER);
    await table();
    const mv = rowOf("perm-clearance-mv");
    expect(within(mv).getByText("Materialized-view deny")).toBeInTheDocument();
    expect(within(mv).queryByRole("link", { name: /Open source/ })).not.toBeInTheDocument();
    const odd = rowOf("perm-something-odd");
    expect(within(odd).getByText("Compiled policy")).toBeInTheDocument();
    expect(within(odd).queryByRole("link", { name: /Open source/ })).not.toBeInTheDocument();
  });

  it("a clearance names its kind instead of a link without tag:read", async () => {
    serve(answers());
    open("", me(["policy:read"]));
    await table();
    const row = rowOf(`perm-clearance-${TAG}`);
    expect(within(row).getByText("Classification clearance")).toBeInTheDocument();
    expect(within(row).queryByRole("link", { name: /Open source/ })).not.toBeInTheDocument();
  });

  it("a seeded row is read-only with its sentence", async () => {
    serve(answers());
    open("", WRITER);
    await table();
    const row = rowOf("egret-admin");
    expect(
      within(row).getByText("Managed by egret (engine delegation, admin access or query limits)."),
    ).toBeInTheDocument();
    expect(within(row).queryByRole("switch")).not.toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /Delete/ })).not.toBeInTheDocument();
  });

  it("a hand-written row toggles, deletes and edits with the permissions", async () => {
    const { asked } = serve(
      answers({
        "PUT /policies/10/enabled": ok({}),
        "DELETE /policies/10": { status: 204, body: null },
      }),
    );
    open("", WRITER);
    await table();
    const row = rowOf("zeta hand-written");
    expect(within(row).getByText("Not managed by egret")).toBeInTheDocument();
    expect(within(row).getByRole("link", { name: "Edit zeta hand-written" })).toHaveAttribute(
      "href",
      `/governance/engines/${E_TR}/advanced/policies/masking/10`,
    );
    await userEvent.click(within(row).getByRole("switch", { name: "Enable zeta hand-written" }));
    await waitFor(() => expect(asked).toContain("PUT /policies/10/enabled"));
    await userEvent.click(within(row).getByRole("button", { name: "Delete zeta hand-written" }));
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.type(within(dialog).getByRole("textbox"), "delete");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(asked).toContain("DELETE /policies/10"));
  });

  it("a tag-service row edits through the main profile's editor with its own type", async () => {
    serve(answers());
    open("", WRITER);
    await table();
    expect(within(rowOf("alpha by tag")).getByRole("link", { name: "Edit alpha by tag" })).toHaveAttribute(
      "href",
      `/governance/engines/${E_TR}/advanced/policies/access/21`,
    );
  });

  it("without policy:update there is no toggle and no Edit; without policy:delete no Delete", async () => {
    serve(answers());
    open("", READER);
    await table();
    const row = rowOf("zeta hand-written");
    expect(within(row).queryByRole("switch")).not.toBeInTheDocument();
    expect(within(row).queryByRole("link", { name: /^Edit/ })).not.toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /Delete/ })).not.toBeInTheDocument();
    expect(within(row).getByText("Enabled")).toBeInTheDocument();
  });

  it("a refused toggle shows the server's reason", async () => {
    serve(answers({ "PUT /policies/10/enabled": fail(409, "the profile is busy") }));
    open("", WRITER);
    await table();
    await userEvent.click(within(rowOf("zeta hand-written")).getByRole("switch", { name: "Enable zeta hand-written" }));
    expect(await screen.findByText("the profile is busy", {}, T)).toBeInTheDocument();
  });

  // Ported from ProfileDetailPage.test.tsx: a 403 names the permission, never echoes "forbidden".
  it("names the permission a refused toggle needs, instead of echoing 'forbidden'", async () => {
    serve(answers({ "PUT /policies/10/enabled": fail(403, "forbidden") }));
    open("", WRITER);
    await table();
    await userEvent.click(within(rowOf("zeta hand-written")).getByRole("switch", { name: "Enable zeta hand-written" }));
    expect(await screen.findByText(/needs the policy:update permission/, {}, T)).toBeInTheDocument();
    expect(screen.queryByText(/^forbidden$/)).not.toBeInTheDocument();
  });
});
