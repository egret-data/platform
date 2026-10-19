import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import type { EngineCapabilitiesResponse, EngineGrantWriteResponse, GovernanceCatalog } from "@/api/generated/model";
import {
  CAPABILITIES,
  E_OTHER,
  E_SR,
  E_TR,
  engineGrant,
  fail,
  grantsResponse,
  created,
  ok,
  openArea,
  policy,
  profileDto,
  READER,
  serve,
  STEWARD,
  type Answer,
} from "./engineHarness";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

beforeEach(() => {
  vi.mocked(toast.success).mockClear();
  vi.mocked(toast.error).mockClear();
  vi.spyOn(HTMLElement.prototype, "scrollIntoView").mockImplementation(() => undefined);
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const T = { timeout: 3000 };

/** The harness's catalogue plus the StarRocks kinds it lacks (`function_signature`, `fixed`). */
const CAPS: EngineCapabilitiesResponse = {
  engines: CAPABILITIES.engines.map((e) =>
    e.engineType !== "starrocks"
      ? e
      : {
          ...e,
          capabilities: [
            ...e.capabilities,
            {
              id: "starrocks.use_global_function",
              label: "Use a global function",
              description: "Call a global function.",
              resources: [{ key: "global_function", valueKind: "function_signature" }],
              access: "usage",
            },
            {
              id: "starrocks.manage_blacklist",
              label: "Manage the SQL blacklist",
              description: "Add and remove blacklist entries.",
              resources: [{ key: "system", valueKind: "fixed" }],
              access: "blacklist",
            },
          ],
        },
  ),
};

const CATALOGS: GovernanceCatalog[] = [
  { id: "c-lonely", name: "lonely", source: "egret", connectorType: "iceberg", engines: [], workspaces: [] },
  { id: "c-ice", name: "ice", source: "egret", connectorType: "iceberg", engines: ["lake-sr", "lake-trino"], workspaces: [] },
  { id: "c-sr", name: "srcat", source: "engine", connectorType: "hive", engines: ["lake-sr"], workspaces: [] },
];

const IMP = engineGrant({
  id: "g-imp-ub",
  capability: "trino.impersonate_user",
  values: ["ub"],
  targetUserId: "u-ub",
  principals: [{ kind: "user", ref: "ua" }],
});

const SR_ENGINE = { id: E_SR, name: "lake-sr", type: "starrocks", origin: "provisioned", status: "running" } as const;

function writeResponse(over: Partial<EngineGrantWriteResponse> = {}): EngineGrantWriteResponse {
  return {
    id: "g-new",
    compile: { services: [2], created: 1, updated: 0, deleted: 0, unchanged: 0, skipped: [] },
    reach: { engines: [{ id: E_TR, name: "lake-trino", origin: "provisioned" }], sharedProfile: false },
    ...over,
  };
}

function answers(over: Record<string, Answer> = {}): Record<string, Answer> {
  return {
    [`/perm/engines/${E_TR}/grants`]: ok(grantsResponse({ grants: [IMP] })),
    [`/perm/engines/${E_SR}/grants`]: ok(
      grantsResponse({
        engine: SR_ENGINE,
        profile: { serviceId: 3, name: "lake-sr" },
        reach: { engines: [{ id: E_SR, name: "lake-sr", origin: "provisioned" }], sharedProfile: false },
      }),
    ),
    "/perm/engine-capabilities": ok(CAPS),
    "/governance/catalogs": ok(CATALOGS),
    "/policies/services": ok([
      profileDto("lake-tr", { engines: [{ id: E_TR, name: "lake-trino", status: "running", origin: "provisioned", type: "trino" }] }),
      profileDto("lake-sr", {
        type: "starrocks",
        engines: [{ id: E_SR, name: "lake-sr", status: "running", origin: "provisioned", type: "starrocks" }],
      }),
    ]),
    "/policies?service=lake-tr": ok([policy(1, "compiler")]),
    "/policies?service=lake-sr": ok([]),
    ...over,
  };
}

async function openGrant() {
  await userEvent.click(await screen.findByRole("button", { name: "Grant" }, T));
  return screen.findByRole("dialog", { name: "Grant engine permission" });
}

async function pickCapability(dialog: HTMLElement, label: string) {
  await userEvent.click(within(dialog).getByRole("combobox", { name: "Capability" }));
  await userEvent.click(await screen.findByRole("option", { name: new RegExp(`^${label}`) }));
}

async function addRole(dialog: HTMLElement, role: string) {
  await userEvent.click(within(dialog).getByRole("combobox", { name: "Include kind" }));
  await userEvent.click(await screen.findByRole("option", { name: "role" }));
  await userEvent.type(within(dialog).getByRole("textbox", { name: "Include role name" }), role);
  await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
}

describe("Grant dialog: who sees it", () => {
  it("offers Grant and Edit only with policy:update; Check with policy:read", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_TR}`, READER);
    await screen.findByText("perm-engine-g-imp-ub", {}, T);
    expect(await screen.findByRole("button", { name: "Check" }, T)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Grant" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Edit/ })).not.toBeInTheDocument();
  });

  it("offers Grant and Edit to a steward", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    expect(await screen.findByRole("button", { name: "Grant" }, T)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Edit engine permission perm-engine-g-imp-ub" })).toBeInTheDocument();
  });
});

describe("Grant dialog: controls per valueKind (spec §6.1)", () => {
  it("lists only the engine type's capabilities, with their descriptions", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    const dialog = await openGrant();
    await userEvent.click(within(dialog).getByRole("combobox", { name: "Capability" }));
    const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(options).toEqual([
      expect.stringContaining("Impersonate a user"),
      expect.stringContaining("Set catalog session properties"),
    ]);
    expect(options[0]).toContain("Run queries as another user.");
  });

  it("takes focus on the first field and closes on Escape", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    const dialog = await openGrant();
    await waitFor(() => expect(within(dialog).getByRole("combobox", { name: "Capability" })).toHaveFocus());
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("username → a user field; catalog → the catalogs a reached engine has; identifier → an input with its rule", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    const dialog = await openGrant();
    await pickCapability(dialog, "Impersonate a user");
    expect(within(dialog).getByLabelText("Target user")).toHaveAttribute("id", "engine-grant-value-trinouser");

    await pickCapability(dialog, "Set catalog session properties");
    expect(within(dialog).queryByLabelText("Target user")).not.toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("combobox", { name: "Catalog" }));
    const catalogs = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(catalogs).toEqual(["ice"]);
    await userEvent.keyboard("{Escape}");
    const input = within(dialog).getByLabelText("Session property");
    expect(input.tagName).toBe("INPUT");
    expect(within(dialog).getByText(/letters, digits and _/)).toBeInTheDocument();
  });

  it("function_signature → an input with an example; fixed → no control, 'Applies to all (*)'", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_SR}`, STEWARD);
    const dialog = await openGrant();
    await pickCapability(dialog, "Use a global function");
    expect(within(dialog).getByLabelText("Function signature").tagName).toBe("INPUT");
    expect(within(dialog).getByText("e.g. my_udf(INT,VARCHAR)")).toBeInTheDocument();

    await pickCapability(dialog, "Manage the SQL blacklist");
    expect(within(dialog).getByText("Applies to all (*)")).toBeInTheDocument();
    expect(within(dialog).queryByLabelText("Function signature")).not.toBeInTheDocument();
    expect(dialog.querySelector('[id^="engine-grant-value-"]')).toBeNull();
  });
});

describe("Grant dialog: client checks (F11 §2)", () => {
  it.each([
    ["  ub", "No leading or trailing spaces."],
    ["u*b", "No *, ?, { or }: values are concrete names."],
    ["u?b", "No *, ?, { or }: values are concrete names."],
    ["{USER}", "No *, ?, { or }: values are concrete names."],
  ])("refuses %j beside the field and sends nothing", async (value, message) => {
    const { asked } = serve(answers());
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    const dialog = await openGrant();
    await pickCapability(dialog, "Impersonate a user");
    await userEvent.type(within(dialog).getByLabelText("Target user"), value.replace(/[{]/g, "{{"));
    await addRole(dialog, "etl");
    await userEvent.click(within(dialog).getByRole("button", { name: "Grant" }));
    const field = within(dialog).getByLabelText("Target user");
    expect(field).toHaveAttribute("aria-invalid", "true");
    expect(document.getElementById(field.getAttribute("aria-describedby")!.split(" ")[0])).toHaveTextContent(message);
    expect(asked.some((a) => a.startsWith("POST"))).toBe(false);
  });

  it("refuses an empty value and no principal", async () => {
    const { asked } = serve(answers());
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    const dialog = await openGrant();
    await pickCapability(dialog, "Impersonate a user");
    await userEvent.click(within(dialog).getByRole("button", { name: "Grant" }));
    expect(within(dialog).getByText("Enter a value.")).toBeInTheDocument();
    expect(within(dialog).getByText("Add at least one user, group or role.")).toBeInTheDocument();
    expect(asked.some((a) => a.startsWith("POST"))).toBe(false);
  });
});

describe("Grant dialog: writes and the three outcomes (Q4)", () => {
  async function fillImpersonation(dialog: HTMLElement) {
    await pickCapability(dialog, "Impersonate a user");
    await userEvent.type(within(dialog).getByLabelText("Target user"), "ub");
    await addRole(dialog, "etl");
  }

  it("creates with the target as a username and no targetUserId; compiled → toast naming the profile", async () => {
    const { bodies, asked } = serve(answers({ [`POST /perm/engines/${E_TR}/grants`]: created(writeResponse()) }));
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    const dialog = await openGrant();
    await fillImpersonation(dialog);
    const grantsBefore = asked.filter((a) => a === `GET /perm/engines/${E_TR}/grants`).length;
    const policiesBefore = asked.filter((a) => a === "GET /policies?service=lake-tr").length;
    await userEvent.click(within(dialog).getByRole("button", { name: "Grant" }));
    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith(
        "Saved and compiled into lake-tr. The engine applies it at its next plugin download.",
      ),
    );
    expect(bodies[`POST /perm/engines/${E_TR}/grants`]).toEqual([
      { capability: "trino.impersonate_user", values: ["ub"], principals: [{ kind: "role", ref: "etl" }] },
    ]);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(() =>
      expect(asked.filter((a) => a === `GET /perm/engines/${E_TR}/grants`).length).toBeGreaterThan(grantsBefore),
    );
    await waitFor(() =>
      expect(asked.filter((a) => a === "GET /policies?service=lake-tr").length).toBeGreaterThan(policiesBefore),
    );
  });

  it("names the other engines the grant reaches", async () => {
    serve(
      answers({
        [`POST /perm/engines/${E_TR}/grants`]: created(
          writeResponse({
            reach: {
              engines: [
                { id: E_TR, name: "lake-trino", origin: "provisioned" },
                { id: E_OTHER, name: "zeta", origin: "external" },
              ],
              sharedProfile: true,
            },
          }),
        ),
      }),
    );
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    const dialog = await openGrant();
    await fillImpersonation(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: "Grant" }));
    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith(
        "Saved and compiled into lake-tr. The engine applies it at its next plugin download. It reaches zeta.",
      ),
    );
  });

  it("saved but not compiled: the tab says so, and no success toast claims it", async () => {
    serve(
      answers({
        [`POST /perm/engines/${E_TR}/grants`]: created(
          writeResponse({
            compile: {
              services: [2],
              created: 0,
              updated: 0,
              deleted: 0,
              unchanged: 3,
              skipped: [
                { source_kind: "grant", id: "g-new", reason: "a data grant of the same id" },
                { source_kind: "engine_grant", id: "g-new", reason: "user ub has no username" },
              ],
            },
          }),
        ),
      }),
    );
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    const dialog = await openGrant();
    await fillImpersonation(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: "Grant" }));
    expect(
      await screen.findByText("Saved, but not compiled: user ub has no username. It is not on the engine.", {}, T),
    ).toBeInTheDocument();
    expect(toast.success).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("not saved: a 409 (a compile failure) keeps the dialog open with the server's reason and every input", async () => {
    serve(
      answers({
        [`POST /perm/engines/${E_TR}/grants`]: fail(409, "the profile failed to compile: target ub is a platform admin"),
      }),
    );
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    const dialog = await openGrant();
    await fillImpersonation(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: "Grant" }));
    expect(
      await within(dialog).findByText("Not saved: the profile failed to compile: target ub is a platform admin", {}, T),
    ).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Grant engine permission" })).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Target user")).toHaveValue("ub");
    expect(within(dialog).getByText("etl")).toBeInTheDocument();
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("shows a refusal of public beside the principals", async () => {
    serve(
      answers({
        [`POST /perm/engines/${E_TR}/grants`]: fail(400, "public cannot hold an engine permission"),
      }),
    );
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    const dialog = await openGrant();
    await fillImpersonation(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: "Grant" }));
    const principals = within(dialog).getByRole("group", { name: "Granted to" });
    expect(
      await within(principals).findByText("Not saved: public cannot hold an engine permission", {}, T),
    ).toBeInTheDocument();
  });

  it("edits with PATCH and the whole body", async () => {
    const { bodies } = serve(
      answers({ "PATCH /perm/engine-grants/g-imp-ub": ok(writeResponse({ id: "g-imp-ub" })) }),
    );
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    await userEvent.click(
      await screen.findByRole("button", { name: "Edit engine permission perm-engine-g-imp-ub" }, T),
    );
    const dialog = await screen.findByRole("dialog", { name: "Edit engine permission" });
    const target = within(dialog).getByLabelText("Target user");
    expect(target).toHaveValue("ub");
    await userEvent.clear(target);
    await userEvent.type(target, "uc");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save changes" }));
    await waitFor(() =>
      expect(bodies["PATCH /perm/engine-grants/g-imp-ub"]).toEqual([
        { capability: "trino.impersonate_user", values: ["uc"], principals: [{ kind: "user", ref: "ua" }] },
      ]),
    );
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
  });
});
