import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import {
  E_TR,
  grantsResponse,
  ok,
  openArea,
  pluginResponse,
  profileDto,
  READER,
  serve,
  STEWARD,
  type Answer,
} from "./engineHarness";
import type { ProfileEngine } from "@/api/generated/model";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

beforeEach(() => vi.mocked(toast.success).mockClear());
afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const RESTART =
  "Every engine bound to this profile restarts its coordinator to load the new access control. Queries running on it fail.";
const EXTERNAL =
  "External engines are not restarted by egret. Their operator must install the new plugin files, and run Trino 483 or later with plugin/opa.";
const PATCH = "PATCH /policies/services/lake-tr";

const PROVISIONED: ProfileEngine = { id: E_TR, name: "lake-trino", status: "running", origin: "provisioned", type: "trino" };
const EXTERNAL_ENGINE: ProfileEngine = {
  id: "7f1c0e8a-0000-4000-8000-0000000000d4",
  name: "ext-trino",
  status: "running",
  origin: "external",
  type: "trino",
};

function answers(
  { mechanism = "ranger", engines = [PROVISIONED] as ProfileEngine[] } = {},
  over: Record<string, Answer> = {},
): Record<string, Answer> {
  return {
    [`/perm/engines/${E_TR}/grants`]: ok(grantsResponse()),
    "/policies/services": ok([profileDto("lake-tr", { id: 2, origin: "generated", mechanism, engines })]),
    "/policies?service=lake-tr": ok([]),
    "/perm/overlaps": ok([]),
    [`/governance/engines/${E_TR}/plugin`]: ok(pluginResponse()),
    ...over,
  };
}

const open = (as = STEWARD) => openArea(`/governance/engines/${E_TR}?tab=advanced`, as);
const section = async () => within(await screen.findByRole("region", { name: "Access control" }, T));

async function chooseOpa(user: ReturnType<typeof userEvent.setup>) {
  const s = await section();
  await user.click(await s.findByLabelText("OPA", {}, T));
}
const dialog = () => screen.findByRole("alertdialog", {}, T);

describe("the Ranger | OPA selector (SP2b Task 23)", () => {
  it("shows the stored mechanism and asks before switching", async () => {
    serve(answers());
    const user = userEvent.setup();
    open();
    const s = await section();
    expect(await s.findByLabelText("Ranger", {}, T)).toBeChecked();
    expect(s.getByRole("radiogroup", { name: "How engines are asked" })).toBeInTheDocument();
    await chooseOpa(user);
    const d = within(await dialog());
    expect(d.getByText("Switch to OPA?")).toBeVisible();
    expect(d.getByText(RESTART)).toBeVisible();
    expect(d.getByRole("button", { name: "Cancel" })).toBeVisible();
    expect(d.getByRole("button", { name: "Switch" })).toBeVisible();
  });

  it("sends nothing when the dialog is cancelled", async () => {
    const served = serve(answers());
    const user = userEvent.setup();
    open();
    await chooseOpa(user);
    await user.click(within(await dialog()).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(served.bodies[PATCH]).toBeUndefined();
    expect((await section()).getByLabelText("Ranger")).toBeChecked();
  });

  it("sends the switch on confirm and reports the reconfigured engines", async () => {
    const served = serve(
      answers(
        {},
        {
          [PATCH]: ok({
            name: "lake-tr",
            mechanism: "opa",
            engines_reconfigured: ["trino-a"],
            external_engines_unchanged: ["ext-trino"],
            external_engines_warning: "ext-trino keeps enforcing the Ranger policies until its OPA plugin files are installed.",
          }),
        },
      ),
    );
    const user = userEvent.setup();
    open();
    await chooseOpa(user);
    await user.click(within(await dialog()).getByRole("button", { name: "Switch" }));
    await waitFor(() => expect(served.bodies[PATCH]).toEqual([{ mechanism: "opa" }]));
    const status = (await section()).getByRole("status");
    await waitFor(() => expect(status).toHaveTextContent("trino-a"));
    expect(status).toHaveTextContent("ext-trino");
    expect(status).toHaveTextContent("keeps enforcing the Ranger policies");
    expect(toast.success).toHaveBeenCalledWith("Switched to OPA.");
    await waitFor(() => expect(served.asked.filter((k) => k === "GET /policies/services").length).toBeGreaterThan(1));
  });

  it("shows every refusal from a 409 and keeps Ranger selected", async () => {
    serve(
      answers(
        {},
        {
          [PATCH]: {
            status: 409,
            body: {
              error: "the switch is refused",
              code: "mechanism_refused",
              refusals: [
                "the profile holds the `all - …` default policies (13); delete them first",
                "OPA is not enabled on this installation",
              ],
            },
          },
        },
      ),
    );
    const user = userEvent.setup();
    open();
    await chooseOpa(user);
    await user.click(within(await dialog()).getByRole("button", { name: "Switch" }));
    const s = await section();
    expect(await s.findByText("the profile holds the `all - …` default policies (13); delete them first", {}, T)).toBeVisible();
    expect(s.getByText("OPA is not enabled on this installation")).toBeVisible();
    expect(s.getByLabelText("Ranger")).toBeChecked();
    expect(s.getByLabelText("OPA")).not.toBeChecked();
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("is read-only without policy:update and says why", async () => {
    serve(answers());
    const user = userEvent.setup();
    open(READER);
    const s = await section();
    expect(await s.findByLabelText("OPA", {}, T)).toBeDisabled();
    expect(s.getByLabelText("Ranger")).toBeDisabled();
    expect(s.getByText("You need policy:update to change this.")).toBeVisible();
    await user.click(s.getByLabelText("OPA"));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("warns about external engines before switching to OPA", async () => {
    serve(answers({ engines: [PROVISIONED, EXTERNAL_ENGINE] }));
    const user = userEvent.setup();
    open();
    await chooseOpa(user);
    const d = within(await dialog());
    expect(d.getByText(RESTART)).toBeVisible();
    expect(d.getByText(EXTERNAL)).toBeVisible();
  });

  it("does not mention external engines when none is bound", async () => {
    serve(answers());
    const user = userEvent.setup();
    open();
    await chooseOpa(user);
    await dialog();
    expect(screen.queryByText(EXTERNAL)).toBeNull();
  });

  it("offers a retry after a 502 and re-sends the same PATCH", async () => {
    const served = serve(
      answers(
        {},
        {
          [PATCH]: {
            status: 502,
            body: {
              error: "the profile is now on opa; engines not reconfigured: trino-b. Send the same PATCH again to retry.",
              engines_reconfigured: ["trino-a"],
              engines_failed: [{ engine: "trino-b", error: "timeout" }],
              external_engines_unchanged: [],
            },
          },
        },
      ),
    );
    const user = userEvent.setup();
    open();
    await chooseOpa(user);
    await user.click(within(await dialog()).getByRole("button", { name: "Switch" }));
    const s = await section();
    expect(await s.findByText(/engines not reconfigured: trino-b/, {}, T)).toBeVisible();
    expect(s.getByText(/trino-b/, { selector: "li" })).toHaveTextContent("timeout");
    await user.click(s.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(served.bodies[PATCH]).toEqual([{ mechanism: "opa" }, { mechanism: "opa" }]));
  });
});
