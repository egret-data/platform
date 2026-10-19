import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { EngineDto, ProfileEngine } from "@/api/generated/model";
import {
  E_OTHER,
  E_SR,
  E_TR,
  engineGrant,
  fail,
  me,
  ok,
  openArea,
  pluginResponse,
  profileDto,
  READER,
  serve,
  type Answer,
} from "./engineHarness";

afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const INDEX = "/governance/engines";

const WITH_ENGINE_READ = me(["policy:read", "engine:read"]);
const DELETER = me(["policy:read", "policy:delete"]);
const CREATOR = me(["policy:read", "policy:create"]);

const bound = (id: string, name: string, type: string, origin: string): ProfileEngine => ({
  id,
  name,
  status: "running",
  origin,
  type,
});

/**
 * Not sorted, and shaped so a plausible wrong reading shows: a tag service with
 * no engine (must not be an orphan), an orphan, and two bound profiles whose
 * hand-written counts differ from their totals.
 */
function profiles() {
  return [
    profileDto("zeta-sr", {
      type: "starrocks",
      policy_count: 5,
      policy_counts: { compiler: 4, infra: 0, operator: 1 },
      engines: [bound(E_SR, "sr-prod", "starrocks", "external")],
    }),
    profileDto("orphan", { origin: "manual", policy_count: 2, policy_counts: { compiler: 0, infra: 2, operator: 0 } }),
    profileDto("lake-tr-tag", { type: "tag", policy_count: 1, policy_counts: { compiler: 0, infra: 0, operator: 1 } }),
    profileDto("lake-tr", {
      policy_count: 9,
      tag_service: "lake-tr-tag",
      policy_counts: { compiler: 5, infra: 1, operator: 3 },
      engines: [bound(E_TR, "lake-trino", "trino", "provisioned")],
    }),
  ];
}

const cpEngine = (id: string, name: string, type: string, origin: string) =>
  ({ id, name, type, origin, namespace: "egret", status: "running", spec: {} }) as unknown as EngineDto;

function answers(over: Record<string, Answer> = {}): Record<string, Answer> {
  return {
    "/policies/services": ok(profiles()),
    "/engines": ok([
      cpEngine(E_OTHER, "unbound-rw", "trino", "provisioned"),
      cpEngine(E_SR, "sr-prod", "starrocks", "external"),
      cpEngine(E_TR, "lake-trino", "trino", "provisioned"),
    ]),
    "/perm/engine-grants": ok([
      engineGrant({ id: "g1", capability: "trino.impersonate_user" }),
      engineGrant({ id: "g2", capability: "starrocks.impersonate_user", engineId: E_SR, engineName: "sr-prod" }),
      engineGrant({ id: "g3", capability: "trino.impersonate_user" }),
    ]),
    [`/governance/engines/${E_TR}/plugin`]: ok(
      pluginResponse({
        kinds: [
          { kind: "policies", currentVersion: 3, state: "up_to_date", sentence: "held" },
          { kind: "tags", currentVersion: 1, state: "behind", sentence: "behind" },
        ],
      }),
    ),
    [`/governance/engines/${E_SR}/plugin`]: ok(
      pluginResponse({ judged: { state: "not_judged", reason: "external", sentence: "egret does not judge external engines." } }),
    ),
    [`/governance/engines/${E_OTHER}/plugin`]: ok(
      pluginResponse({ profile: null, judged: { state: "not_judged", reason: "no_profile", sentence: "No profile." } }),
    ),
    ...over,
  };
}

const enginesSection = () => screen.getByRole("region", { name: "Engines" });
const orphansSection = () => screen.getByRole("region", { name: "Profiles without an engine" });
const rowOf = (section: HTMLElement, name: string) => {
  const link = within(section).getByRole("link", { name });
  const row = link.closest("tr");
  if (!row) throw new Error(`no row for ${name}`);
  return row as HTMLElement;
};

/** The text of `row`'s cell under the column headed `label` in `section`'s table. */
const cellText = (section: HTMLElement, row: HTMLElement, label: string) => {
  const heads = within(section).getAllByRole("columnheader").map((h) => h.textContent?.trim());
  const i = heads.indexOf(label);
  if (i < 0) throw new Error(`no column ${label} in ${heads.join(", ")}`);
  return within(row).getAllByRole("cell")[i].textContent?.trim();
};

describe("the Engine & policy index (spec §5)", () => {
  it("is a list page headed Engine & policy, with counts that write their own plurals", async () => {
    serve(answers());
    openArea(INDEX, WITH_ENGINE_READ);
    expect(await screen.findByRole("heading", { level: 1, name: "Engine & policy" }, T)).toBeInTheDocument();
    // 3 engines (one unbound), 1 orphan (the tag service is not one), 3 + 1 hand-written
    // (the tag service's own row is already inside lake-tr's count, so it is not added twice).
    expect(await screen.findByText("engines", {}, T)).toBeInTheDocument();
    const strip = screen.getByText("engines").parentElement!.parentElement!;
    expect(strip).toHaveTextContent("3engines");
    expect(strip).toHaveTextContent("1profile without an engine");
    expect(strip).toHaveTextContent("4hand-written policies");
  });

  it("with engine:read lists every engine, an unbound one as No profile, with plugin state and grant counts", async () => {
    serve(answers());
    openArea(INDEX, WITH_ENGINE_READ);
    await screen.findByRole("link", { name: "unbound-rw" }, T);
    const section = enginesSection();
    const names = within(section)
      .getAllByRole("link")
      .map((a) => a.textContent)
      .filter((t) => ["lake-trino", "sr-prod", "unbound-rw"].includes(t ?? ""));
    expect(names).toEqual(["lake-trino", "sr-prod", "unbound-rw"]);

    const tr = rowOf(section, "lake-trino");
    expect(within(tr).getByRole("link", { name: "lake-trino" })).toHaveAttribute("href", `${INDEX}/${E_TR}`);
    expect(within(tr).getByText("Provisioned")).toBeInTheDocument();
    expect(within(tr).getByText("lake-tr")).toBeInTheDocument();
    // The worst kind state of the plugin route wins: tags behind beats policies up to date.
    expect(await within(tr).findByText("Behind", {}, T)).toBeInTheDocument();
    expect(cellText(section, tr, "Engine grants")).toBe("2");
    expect(cellText(section, tr, "Hand-written policies")).toBe("3");

    const sr = rowOf(section, "sr-prod");
    expect(within(sr).getByText("External")).toBeInTheDocument();
    expect(await within(sr).findByText("Not judged: external", {}, T)).toBeInTheDocument();
    expect(cellText(section, sr, "Engine grants")).toBe("1");
    expect(cellText(section, sr, "Hand-written policies")).toBe("1");

    const un = rowOf(section, "unbound-rw");
    expect(within(un).getByText("No profile")).toBeInTheDocument();
    expect(within(un).getByRole("link", { name: "Bind a profile in Control plane" })).toHaveAttribute(
      "href",
      `/control-plane/engines/${E_OTHER}?tab=settings`,
    );
    expect(cellText(section, un, "Engine grants")).toBe("0");
  });

  it("without engine:read lists only the bound engines, from the profiles, with no plugin column and no engines request", async () => {
    const served = serve(answers());
    openArea(INDEX, READER);
    await screen.findByRole("link", { name: "lake-trino" }, T);
    const section = enginesSection();
    expect(within(section).queryByRole("link", { name: "unbound-rw" })).toBeNull();
    expect(within(section).queryByRole("columnheader", { name: /plugin/i })).toBeNull();
    const sr = rowOf(section, "sr-prod");
    expect(within(sr).getByText("StarRocks")).toBeInTheDocument();
    expect(within(sr).getByText("External")).toBeInTheDocument();
    expect(within(sr).getByText("zeta-sr")).toBeInTheDocument();
    expect(served.asked.some((k) => k === "GET /engines")).toBe(false);
    expect(served.asked.some((k) => k.includes("/plugin"))).toBe(false);
  });

  it("lists an unbound profile under Profiles without an engine, never a tag service or a bound profile", async () => {
    serve(answers());
    openArea(INDEX, READER);
    const link = await within(await screen.findByRole("region", { name: "Profiles without an engine" }, T)).findByRole(
      "link",
      { name: "orphan" },
      T,
    );
    expect(link).toHaveAttribute("href", `${INDEX}/profiles/orphan`);
    const section = orphansSection();
    expect(within(section).queryByText("lake-tr-tag")).toBeNull();
    expect(within(section).queryByText("lake-tr")).toBeNull();
    expect(within(section).queryByText("zeta-sr")).toBeNull();
    const row = rowOf(section, "orphan");
    expect(within(row).getByText("Manual")).toBeInTheDocument();
    expect(cellText(section, row, "Policies")).toBe("2");
  });

  it("says every profile is bound when none is unbound", async () => {
    serve(answers({ "/policies/services": ok(profiles().filter((p) => p.name !== "orphan")) }));
    openArea(INDEX, READER);
    expect(await screen.findByText("Every profile is bound to an engine.", {}, T)).toBeInTheDocument();
  });

  it("says no engine is recorded when there is none", async () => {
    serve(answers({ "/engines": ok([]), "/policies/services": ok([]) }));
    openArea(INDEX, WITH_ENGINE_READ);
    expect(
      await screen.findByText("No engine is recorded. Engines are created in Control plane.", {}, T),
    ).toBeInTheDocument();
  });
});

describe("deleting a profile without an engine", () => {
  it("is not offered without policy:delete", async () => {
    serve(answers());
    openArea(INDEX, READER);
    await within(await screen.findByRole("region", { name: "Profiles without an engine" }, T)).findByRole(
      "link",
      { name: "orphan" },
      T,
    );
    expect(screen.queryByRole("button", { name: /delete/i })).toBeNull();
  });

  it("is confirmed by typing the profile's name, and the list is read again", async () => {
    const a = answers({ "DELETE /policies/services/orphan": { status: 204, body: null } });
    const served = serve(a);
    openArea(INDEX, DELETER);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Delete orphan" }, T));
    const dialog = await screen.findByRole("alertdialog");
    const confirm = within(dialog).getByRole("button", { name: "Delete" });
    const input = within(dialog).getByRole("textbox");
    await user.type(input, "delete");
    expect(confirm).toBeDisabled();
    await user.clear(input);
    await user.type(input, "orpha");
    expect(confirm).toBeDisabled();
    await user.type(input, "n");
    expect(confirm).toBeEnabled();
    a["/policies/services"] = ok(profiles().filter((p) => p.name !== "orphan"));
    await user.click(confirm);
    await waitFor(() => expect(served.asked).toContain("DELETE /policies/services/orphan"));
    expect(await screen.findByText("Every profile is bound to an engine.", {}, T)).toBeInTheDocument();
  });

  it("shows the server's refusal", async () => {
    serve(
      answers({
        "DELETE /policies/services/orphan": {
          status: 409,
          body: { error: "profile orphan is generated; it cannot be deleted" },
        },
      }),
    );
    openArea(INDEX, DELETER);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Delete orphan" }, T));
    const dialog = await screen.findByRole("alertdialog");
    await user.type(within(dialog).getByRole("textbox"), "orphan");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await screen.findByText("profile orphan is generated; it cannot be deleted", {}, T)).toBeInTheDocument();
    expect(within(orphansSection()).getByRole("link", { name: "orphan" })).toBeInTheDocument();
  });
  // Ported from Policies.test.tsx: a 403 names the permission, never echoes "forbidden".
  it("names the permission a refused delete needs, not 'forbidden'", async () => {
    serve(answers({ "DELETE /policies/services/orphan": { status: 403, body: { error: "forbidden" } } }));
    openArea(INDEX, DELETER);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Delete orphan" }, T));
    const dialog = await screen.findByRole("alertdialog");
    await user.type(within(dialog).getByRole("textbox"), "orphan");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await screen.findByText(/needs the policy:delete permission/, {}, T)).toBeInTheDocument();
    expect(screen.queryByText(/^forbidden$/)).not.toBeInTheDocument();
  });
});

describe("creating a profile", () => {
  it("is not offered without policy:create", async () => {
    serve(answers());
    openArea(INDEX, READER);
    await screen.findByRole("link", { name: "lake-trino" }, T);
    expect(screen.queryByRole("button", { name: "New profile" })).toBeNull();
  });

  it("creates the profile, which then appears among the unbound ones", async () => {
    const a = answers({
      "POST /policies/services": ok({ id: 9, name: "sr-ext", type: "starrocks" }),
    });
    const served = serve(a);
    openArea(INDEX, CREATOR);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "New profile" }, T));
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByLabelText("Name"), "sr-ext");
    a["/policies/services"] = ok([...profiles(), profileDto("sr-ext", { type: "starrocks", origin: "manual" })]);
    await user.click(within(dialog).getByRole("button", { name: "Create profile" }));
    await waitFor(() => expect(served.bodies["POST /policies/services"]).toEqual([{ name: "sr-ext", type: "trino" }]));
    expect(await within(orphansSection()).findByRole("link", { name: "sr-ext" }, T)).toBeInTheDocument();
  });

  it("keeps the dialog open with the server's message on a refusal", async () => {
    serve(answers({ "POST /policies/services": { status: 409, body: { error: "a profile named orphan exists" } } }));
    openArea(INDEX, CREATOR);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "New profile" }, T));
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByLabelText("Name"), "orphan");
    await user.click(within(dialog).getByRole("button", { name: "Create profile" }));
    expect(await within(dialog).findByText("a profile named orphan exists", {}, T)).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Name")).toHaveValue("orphan");
  });

  // Ported from Policies.test.tsx: the create dialog's own 403.
  it("names the permission a refused create needs, not 'forbidden'", async () => {
    serve(answers({ "POST /policies/services": { status: 403, body: { error: "forbidden" } } }));
    openArea(INDEX, CREATOR);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "New profile" }, T));
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByLabelText("Name"), "sr-2");
    await user.click(within(dialog).getByRole("button", { name: "Create profile" }));
    expect(await within(dialog).findByText(/needs the policy:create permission/, {}, T)).toBeInTheDocument();
  });
});

describe("each section's states (FE-STATE-1)", () => {
  it("shows skeletons while loading, not empty sentences", async () => {
    serve(answers({ "/policies/services": "never", "/engines": "never" }));
    const { container } = openArea(INDEX, WITH_ENGINE_READ);
    await screen.findByRole("heading", { level: 1, name: "Engine & policy" }, T);
    expect(container.querySelectorAll('[data-slot="skeleton"], .animate-pulse').length).toBeGreaterThan(0);
    expect(screen.queryByText("Every profile is bound to an engine.")).toBeNull();
    expect(screen.queryByText(/No engine is recorded/)).toBeNull();
  });

  it("a failing engines read leaves the profiles section rendered, and Retry reads again", async () => {
    const a = answers({ "/engines": fail(500) });
    const served = serve(a);
    openArea(INDEX, WITH_ENGINE_READ);
    const section = await screen.findByRole("region", { name: "Engines" }, T);
    const retry = await within(section).findByRole("button", { name: "Retry" }, T);
    expect(within(orphansSection()).getByRole("link", { name: "orphan" })).toBeInTheDocument();
    a["/engines"] = ok([cpEngine(E_TR, "lake-trino", "trino", "provisioned")]);
    await userEvent.setup().click(retry);
    expect(await within(enginesSection()).findByRole("link", { name: "lake-trino" }, T)).toBeInTheDocument();
    expect(served.asked.filter((k) => k === "GET /engines").length).toBeGreaterThan(1);
  });

  it("a failing profiles read shows its error with Retry and keeps the engine table", async () => {
    serve(answers({ "/policies/services": fail(500) }));
    openArea(INDEX, WITH_ENGINE_READ);
    const section = await screen.findByRole("region", { name: "Profiles without an engine" }, T);
    expect(await within(section).findByRole("button", { name: "Retry" }, T)).toBeInTheDocument();
    expect(await within(enginesSection()).findByRole("link", { name: "lake-trino" }, T)).toBeInTheDocument();
  });
});
