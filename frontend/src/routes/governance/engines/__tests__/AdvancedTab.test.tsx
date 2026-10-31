import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import {
  E_SR,
  E_TR,
  fail,
  grantsResponse,
  me,
  ok,
  openArea,
  pluginResponse,
  policy,
  profileDto,
  READER,
  serve,
  where,
  type Answer,
} from "./engineHarness";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

beforeEach(() => vi.mocked(toast.success).mockClear());
afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const WRITER = me(["policy:read", "policy:create", "policy:update", "policy:delete", "policy:seed"]);

const WARNING =
  "Manual policies are outside egret's model: egret does not recompile them, does not check overlaps with grants, and Data and People pages will mark them manual. Prefer Engine permissions or grants on Data.";
const GENERATED_SENTENCE =
  "egret compiles this profile, so new manual policies are refused. Existing ones can still be edited, enabled, disabled and deleted.";

/** Not pre-sorted; one row of each source, the hand-written ones in the profile and in its tag service. */
const ROWS = [
  policy(3, "compiler", { name: "perm-grant-7f1c0e8a-0000-4000-8000-000000000001" }),
  policy(4, "operator", { name: "zeta hand mask", policy_type: "masking", resource_path: "catalog=ice;column=email" }),
  policy(5, "infra", { name: "egret-query-timeout" }),
  policy(6, "operator", { name: "alpha hand access", is_enabled: false }),
];

function answers(
  { origin = "manual", type = "trino", tagService = null as string | null } = {},
  over: Record<string, Answer> = {},
): Record<string, Answer> {
  return {
    [`/perm/engines/${E_TR}/grants`]: ok(grantsResponse()),
    "/policies/services": ok([
      profileDto("lake-tr", {
        id: 2,
        type,
        origin,
        tag_service: tagService,
        engines: [{ id: E_TR, name: "lake-trino", status: "running", origin: "provisioned", type: "trino" }],
      }),
      ...(tagService ? [profileDto(tagService, { id: 1, type: "tag", origin })] : []),
    ]),
    "/policies?service=lake-tr": ok(ROWS),
    ...(tagService ? { [`/policies?service=${tagService}`]: ok([policy(9, "operator", { name: "by tag pii", service: tagService })]) } : {}),
    "/perm/overlaps": ok([]),
    [`/governance/engines/${E_TR}/plugin`]: ok(pluginResponse()),
    ...over,
  };
}

const open = (as = WRITER) => openArea(`/governance/engines/${E_TR}?tab=advanced`, as);
const panel = () => screen.findByRole("tabpanel", {}, T);

describe("the Advanced tab (spec §6.6, EP2, EP9)", () => {
  it("always shows the fixed warning, for a reader too", async () => {
    serve(answers());
    open(READER);
    expect(await within(await panel()).findByText(WARNING, {}, T)).toBeInTheDocument();
  });

  it("lists only the manual policies, each linking to its editor", async () => {
    serve(answers({ tagService: "lake-tr-tag" }));
    open();
    const p = await panel();
    const list = await within(p).findByRole("table", { name: "Manual policies" }, T);
    const names = within(list)
      .getAllByRole("link")
      .map((a) => a.textContent);
    expect(names).toEqual(["alpha hand access", "by tag pii", "zeta hand mask"]);
    expect(within(list).getByRole("link", { name: "zeta hand mask" })).toHaveAttribute(
      "href",
      `/governance/engines/${E_TR}/advanced/policies/masking/4`,
    );
    expect(within(list).queryByText("egret-query-timeout")).toBeNull();
    expect(within(list).queryByText(/perm-grant-/)).toBeNull();
    expect(within(list).getByText("Disabled")).toBeInTheDocument();
  });

  it("says when the profile has no manual policy", async () => {
    serve(answers({}, { "/policies?service=lake-tr": ok([policy(3, "compiler")]) }));
    open();
    expect(await within(await panel()).findByText("This profile has no manual policy.", {}, T)).toBeInTheDocument();
    expect(within(await panel()).queryByText(/hand-written/i)).not.toBeInTheDocument();
  });

  it("offers Create policy on a manual profile: Access, Masking, Row filter, as links to the editor", async () => {
    serve(answers());
    open();
    const p = await panel();
    await userEvent.click(await within(p).findByRole("button", { name: "Create policy" }, T));
    const items = await screen.findAllByRole("menuitem", {}, T);
    expect(items.map((i) => i.textContent)).toEqual(["Access", "Masking", "Row filter"]);
    expect(screen.getByRole("menuitem", { name: "Masking" })).toHaveAttribute(
      "href",
      `/governance/engines/${E_TR}/advanced/policies/masking/new`,
    );
  });

  it("adds By tag when the profile has a tag service", async () => {
    serve(answers({ tagService: "lake-tr-tag" }));
    open();
    const p = await panel();
    await userEvent.click(await within(p).findByRole("button", { name: "Create policy" }, T));
    expect(await screen.findByRole("menuitem", { name: "By tag" }, T)).toHaveAttribute(
      "href",
      `/governance/engines/${E_TR}/advanced/policies/access/new?scope=tag`,
    );
  });

  it("hides Create policy without policy:create", async () => {
    serve(answers());
    open(me(["policy:read", "policy:seed"]));
    const p = await panel();
    await within(p).findByText(WARNING, {}, T);
    await within(p).findByRole("button", { name: "Seed default policies" }, T);
    expect(within(p).queryByRole("button", { name: "Create policy" })).toBeNull();
  });

  it("on a generated profile offers neither Create nor Seed, and says why", async () => {
    serve(answers({ origin: "generated" }));
    open();
    const p = await panel();
    expect(await within(p).findByText(GENERATED_SENTENCE, {}, T)).toBeInTheDocument();
    expect(within(p).queryByRole("button", { name: "Create policy" })).toBeNull();
    expect(within(p).queryByRole("button", { name: "Seed default policies" })).toBeNull();
  });

  it("offers Seed default policies on a manual Trino profile with policy:seed, explains it, and reports the count", async () => {
    const served = serve(
      answers({}, { "POST /policies/services/lake-tr/seed-defaults": ok({ created: 3 }) }),
    );
    open();
    const p = await panel();
    expect(
      await within(p).findByText(
        "Adds egret's default Trino set (group admin: all access). Existing policies are not changed.",
        {},
        T,
      ),
    ).toBeInTheDocument();
    await userEvent.click(within(p).getByRole("button", { name: "Seed default policies" }));
    await waitFor(() => expect(served.asked).toContain("POST /policies/services/lake-tr/seed-defaults"));
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Added 3 default policies."));
  });

  // Ported from ProfileDetailPage.test.tsx: a platform admin holds every write.
  it("offers Create policy and Seed default policies to a platform_admin", async () => {
    serve(answers());
    open(me([], ["platform_admin"]));
    const p = await panel();
    expect(await within(p).findByRole("button", { name: "Create policy" }, T)).toBeInTheDocument();
    expect(within(p).getByRole("button", { name: "Seed default policies" })).toBeInTheDocument();
  });

  it("offers no Seed without policy:seed", async () => {
    serve(answers());
    open(me(["policy:read", "policy:create"]));
    const p = await panel();
    await within(p).findByRole("button", { name: "Create policy" }, T);
    expect(within(p).queryByRole("button", { name: "Seed default policies" })).toBeNull();
  });

  it("offers no Seed on a StarRocks profile", async () => {
    serve({
      [`/perm/engines/${E_SR}/grants`]: ok(
        grantsResponse({
          engine: { id: E_SR, name: "lake-sr", type: "starrocks", origin: "provisioned", status: "running" },
          profile: { serviceId: 3, name: "lake-sr" },
          reach: { engines: [{ id: E_SR, name: "lake-sr", origin: "provisioned" }], sharedProfile: false },
        }),
      ),
      "/policies/services": ok([
        profileDto("lake-sr", {
          id: 3,
          type: "starrocks",
          origin: "manual",
          engines: [{ id: E_SR, name: "lake-sr", status: "running", origin: "provisioned", type: "starrocks" }],
        }),
      ]),
      "/policies?service=lake-sr": ok([]),
      "/perm/overlaps": ok([]),
    });
    openArea(`/governance/engines/${E_SR}?tab=advanced`, WRITER);
    const p = await panel();
    await within(p).findByRole("button", { name: "Create policy" }, T);
    expect(within(p).queryByRole("button", { name: "Seed default policies" })).toBeNull();
  });

  it("shows the seed refusal in the server's words", async () => {
    serve(answers({}, { "POST /policies/services/lake-tr/seed-defaults": fail(409, "profile lake-tr is compiled") }));
    open();
    const p = await panel();
    await userEvent.click(await within(p).findByRole("button", { name: "Seed default policies" }, T));
    expect(await within(p).findByText("profile lake-tr is compiled", {}, T)).toBeInTheDocument();
  });

  it("shows the policy list's error with Retry", async () => {
    serve(answers({}, { "/policies?service=lake-tr": fail(500) }));
    open();
    const p = await panel();
    expect(await within(p).findByRole("button", { name: "Retry" }, T)).toBeInTheDocument();
    expect(within(p).getByText(WARNING)).toBeInTheDocument();
  });
});

const SERVICE_DEF = {
  accessTypes: [{ name: "select" }],
  resources: [
    { name: "catalog", mandatory: true },
    { name: "schema", parent: "catalog" },
    { name: "table", parent: "schema" },
    { name: "column", parent: "table" },
  ],
  dataMaskDef: { resources: [{ name: "catalog" }, { name: "schema" }, { name: "table" }, { name: "column" }] },
  rowFilterDef: { resources: [{ name: "catalog" }, { name: "schema" }, { name: "table" }] },
};
const STORED = {
  name: "alpha hand access",
  isEnabled: true,
  resources: { catalog: { values: ["ice"] } },
  policyItems: [],
  denyPolicyItems: [],
};

describe("the policy editor under the Advanced tab (spec §2, Q2)", () => {
  it("edits a policy of an engine's profile, saves, and returns to the Advanced tab", async () => {
    const served = serve(
      answers({}, {
        "/policies/service-def/lake-tr": ok(SERVICE_DEF),
        "/policies/6": ok(STORED),
        "PUT /policies/6": ok({ id: 6 }),
      }),
    );
    openArea(`/governance/engines/${E_TR}/advanced/policies/access/6`, WRITER);
    expect(await screen.findByDisplayValue("alpha hand access", {}, T)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /lake-tr/ })).toHaveAttribute("href", `/governance/engines/${E_TR}?tab=advanced`);
    await userEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(served.asked).toContain("PUT /policies/6"));
    await waitFor(() => expect(where()).toBe(`/governance/engines/${E_TR}?tab=advanced`));
  });

  it("opens a new masking policy for an unbound profile and goes back to its Advanced tab", async () => {
    serve({
      "/policies/services": ok([profileDto("loose", { id: 5, origin: "manual" })]),
      "/policies/service-def/loose": ok(SERVICE_DEF),
      "/policies?service=loose": ok([]),
      "/perm/overlaps": ok([]),
    });
    openArea("/governance/engines/profiles/loose/advanced/policies/masking/new", WRITER);
    expect(await screen.findByLabelText("Policy name", {}, T)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("link", { name: /loose/ }));
    await waitFor(() => expect(where()).toBe("/governance/engines/profiles/loose?tab=advanced"));
  });

  it("keeps ?scope=tag: the by-tag editor opens", async () => {
    serve(answers({ tagService: "lake-tr-tag" }, { "/policies/service-def/lake-tr": ok(SERVICE_DEF) }));
    openArea(`/governance/engines/${E_TR}/advanced/policies/access/new?scope=tag`, WRITER);
    expect(await screen.findByLabelText("Tag", {}, T)).toBeInTheDocument();
  });

  it("says so when the engine has no profile to write a policy in", async () => {
    serve({ [`/perm/engines/${E_TR}/grants`]: ok(grantsResponse({ profile: null })) });
    openArea(`/governance/engines/${E_TR}/advanced/policies/access/new`, WRITER);
    expect(
      await screen.findByText("This engine downloads no policy profile, so it has no policy to edit.", {}, T),
    ).toBeInTheDocument();
  });

  it("opens an egret-seeded policy read-only, as the editor always has", async () => {
    serve(
      answers({}, {
        "/policies/service-def/lake-tr": ok(SERVICE_DEF),
        "/policies/5": ok({ ...STORED, name: "egret-query-timeout" }),
        "PUT /policies/5": fail(409, "conflict: policy egret-query-timeout is managed by egret (infra)"),
      }),
    );
    openArea(`/governance/engines/${E_TR}/advanced/policies/access/5`, WRITER);
    await screen.findByDisplayValue("egret-query-timeout", {}, T);
    await userEvent.click(screen.getByRole("button", { name: /^save$/i }));
    expect(await screen.findByText(/managed by egret/i, {}, T)).toBeInTheDocument();
    expect(where()).toBe(`/governance/engines/${E_TR}/advanced/policies/access/5`);
  });
});

// KI-135: seeding says what it did, on the tab, and the empty list says where seeded rows are.
describe("Seed default policies says what happened (KI-135)", () => {
  it("keeps the count on the tab and says the seeded policies are on Policies", async () => {
    const a = answers({}, {
      "/policies?service=lake-tr": ok([]),
      "POST /policies/services/lake-tr/seed-defaults": ok({ created: 13 }),
    });
    serve(a);
    open();
    const p = await panel();
    expect(await within(p).findByText("This profile has no manual policy.", {}, T)).toBeInTheDocument();
    a["/policies?service=lake-tr"] = ok(Array.from({ length: 13 }, (_, i) => policy(20 + i, "infra")));
    await userEvent.click(within(p).getByRole("button", { name: "Seed default policies" }));
    const status = await within(p).findByRole("status", {}, T);
    expect(status).toHaveTextContent(
      "Added 13 default policies. They are egret-seeded, so they are listed on the Policies tab, not here.",
    );
    expect(
      await within(p).findByText("Its 13 egret-seeded policies are listed on the Policies tab.", {}, T),
    ).toBeInTheDocument();
  });

  it("says why nothing was added", async () => {
    serve(answers({}, { "POST /policies/services/lake-tr/seed-defaults": ok({ created: 0 }) }));
    open();
    const p = await panel();
    await userEvent.click(await within(p).findByRole("button", { name: "Seed default policies" }, T));
    expect(await within(p).findByRole("status", {}, T)).toHaveTextContent(
      "No default policy was added: the profile already has policies.",
    );
  });

  it("names the egret-compiled and egret-seeded policies when there is no manual one", async () => {
    serve(answers({}, { "/policies?service=lake-tr": ok([policy(3, "compiler"), policy(4, "compiler"), policy(5, "infra")]) }));
    open();
    expect(
      await within(await panel()).findByText(
        "Its 2 egret-compiled policies and 1 egret-seeded policy are listed on the Policies tab.",
        {},
        T,
      ),
    ).toBeInTheDocument();
  });
});

// #786 / KI-125: the tag service of an unbound profile is set on its Advanced tab.
describe("the tag service section (#786)", () => {
  const unbound = (over: Partial<Parameters<typeof profileDto>[1]> = {}) => [
    profileDto("loose", { id: 5, origin: "manual", ...over }),
    profileDto("loose_tag", { id: 6, type: "tag", origin: "manual" }),
    profileDto("other-tr", { id: 7, origin: "manual" }),
  ];
  const base = (profiles = unbound()): Record<string, Answer> => ({
    "/policies/services": ok(profiles),
    "/policies?service=loose": ok([]),
    "/policies?service=loose_tag": ok([]),
    "/perm/overlaps": ok([]),
  });
  const openLoose = (as = WRITER) => openArea("/governance/engines/profiles/loose?tab=advanced", as);

  it("sets it from the existing tag profiles only, and says the engine can now be bound", async () => {
    const a = base();
    a["PUT /policies/services/loose/tag-service"] = ok({ name: "loose", tagService: "loose_tag" });
    const served = serve(a);
    openLoose();
    const p = await panel();
    const section = await within(p).findByRole("region", { name: "Tag service" }, T);
    expect(within(section).getByText(/has no tag service, so no engine can be bound to it/)).toBeInTheDocument();
    await userEvent.click(within(section).getByRole("combobox", { name: "Tag service" }));
    const options = (await screen.findAllByRole("option", {}, T)).map((o) => o.textContent);
    expect(options).toEqual(["None", "loose_tag"]);
    await userEvent.click(screen.getByRole("option", { name: "loose_tag" }));
    a["/policies/services"] = ok(unbound({ tag_service: "loose_tag" }));
    await userEvent.click(within(section).getByRole("button", { name: "Save tag service" }));
    await waitFor(() =>
      expect(served.bodies["PUT /policies/services/loose/tag-service"]).toEqual([{ tagService: "loose_tag" }]),
    );
    expect(await within(section).findByRole("status", {}, T)).toHaveTextContent("Tag service set to loose_tag.");
  });

  it("shows the server's refusal in the section", async () => {
    const a = base();
    a["PUT /policies/services/loose/tag-service"] = fail(400, '"tagService" must name a `tag` profile');
    serve(a);
    openLoose();
    const section = await within(await panel()).findByRole("region", { name: "Tag service" }, T);
    await userEvent.click(within(section).getByRole("combobox", { name: "Tag service" }));
    await userEvent.click(await screen.findByRole("option", { name: "loose_tag" }, T));
    await userEvent.click(within(section).getByRole("button", { name: "Save tag service" }));
    expect(await within(section).findByText(/must name a `tag` profile/, {}, T)).toBeInTheDocument();
  });

  it("is read-only on an engine's profile, and says why", async () => {
    serve(answers({ tagService: "lake-tr-tag" }));
    open();
    const section = await within(await panel()).findByRole("region", { name: "Tag service" }, T);
    expect(within(section).getByText("lake-tr-tag")).toBeInTheDocument();
    expect(within(section).getByText(/Unbind the engine to change it/)).toBeInTheDocument();
    expect(within(section).queryByRole("combobox")).toBeNull();
  });

  it("is read-only without policy:update", async () => {
    serve(base());
    openLoose(me(["policy:read"]));
    const section = await within(await panel()).findByRole("region", { name: "Tag service" }, T);
    expect(within(section).queryByRole("combobox")).toBeNull();
  });

  it("is not shown on a StarRocks profile", async () => {
    serve(base([profileDto("loose", { id: 5, type: "starrocks", origin: "manual" })]));
    openLoose();
    const p = await panel();
    await within(p).findByText(WARNING, {}, T);
    expect(within(p).queryByRole("region", { name: "Tag service" })).toBeNull();
  });
});
