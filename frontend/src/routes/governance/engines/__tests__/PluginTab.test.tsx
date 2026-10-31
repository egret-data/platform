import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { EnginePluginResponse, KindReport } from "@/api/generated/model";
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
  type Answer,
} from "./engineHarness";

afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const ENGINE_READER = me(["policy:read", "engine:read"]);
const PLUGIN = `/governance/engines/${E_TR}/plugin`;

function kind(over: Partial<KindReport> & Pick<KindReport, "kind" | "state">): KindReport {
  return {
    currentVersion: 7,
    lastDownloadAt: "2026-10-19T09:58:00Z",
    newestReportedVersion: 7,
    servedVersion: 7,
    notReported: null,
    sentence: `sentence for ${over.kind} ${over.state}`,
    ...over,
  };
}

function answers(plugin: Answer, over: Record<string, Answer> = {}): Record<string, Answer> {
  return {
    [`/perm/engines/${E_TR}/grants`]: ok(grantsResponse()),
    "/policies/services": ok([
      profileDto("lake-tr", {
        id: 2,
        engines: [{ id: E_TR, name: "lake-trino", status: "running", origin: "provisioned", type: "trino" }],
      }),
    ]),
    "/policies?service=lake-tr": ok([policy(1, "compiler")]),
    "/perm/overlaps": ok([]),
    [PLUGIN]: plugin,
    ...over,
  };
}

const open = (as = ENGINE_READER, id = E_TR) => openArea(`/governance/engines/${id}?tab=plugin`, as);
const panel = () => screen.getByRole("tabpanel");

describe("the Plugin tab (spec §6.5, EP17–EP19)", () => {
  it("without engine:read says which permission it needs and asks the plugin route nothing", async () => {
    const served = serve(answers(ok(pluginResponse())));
    open(READER);
    expect(
      await within(await screen.findByRole("tabpanel", {}, T)).findByText(
        "Plugin state needs the engine:read permission.",
        {},
        T,
      ),
    ).toBeInTheDocument();
    expect(served.asked.some((a) => a.includes("/plugin"))).toBe(false);
  });

  it("shows the plugin version and whether egret models it, or the note", async () => {
    serve(answers(ok(pluginResponse({ plugin: { version: "2.5.0", modelled: false, note: "Ranger 2.4 is not modelled" } }))));
    open();
    const p = await screen.findByRole("tabpanel", {}, T);
    expect(await within(p).findByText("2.5.0", {}, T)).toBeInTheDocument();
    expect(within(p).getByText("Ranger 2.4 is not modelled")).toBeInTheDocument();
    expect(within(p).queryByText("Modelled by egret")).toBeNull();
  });

  it("says a modelled plugin is modelled", async () => {
    serve(answers(ok(pluginResponse())));
    open();
    expect(await within(await screen.findByRole("tabpanel", {}, T)).findByText("Modelled by egret", {}, T)).toBeInTheDocument();
  });

  const STATES: Array<[KindReport, string]> = [
    [kind({ kind: "policies", state: "up_to_date" }), "Up to date"],
    [kind({ kind: "policies", state: "pending" }), "Pending"],
    [kind({ kind: "policies", state: "behind", newestReportedVersion: 5 }), "Behind"],
    [kind({ kind: "policies", state: "quiet" }), "Quiet"],
    [kind({ kind: "policies", state: "silent" }), "Silent"],
    [kind({ kind: "policies", state: "not_reported", notReported: "no_download", lastDownloadAt: null }), "Not reported: no download in 24 hours"],
    [kind({ kind: "policies", state: "not_reported", notReported: "version_unreported", newestReportedVersion: null }), "Not reported: no version reported yet"],
  ];

  it.each(STATES)("renders a %# kind row with its state word, an icon and the server's sentence", async (k, word) => {
    serve(answers(ok(pluginResponse({ kinds: [k] }))));
    open();
    const row = await within(await screen.findByRole("tabpanel", {}, T)).findByRole("row", { name: /Policies/ }, T);
    expect(within(row).getByText(word)).toBeInTheDocument();
    // Never colour alone (FE-A11Y-4): the word is there, and an icon beside it.
    expect(row.querySelector("svg")).not.toBeNull();
    expect(within(row).getByText(k.sentence)).toBeInTheDocument();
  });

  it("renders a tags kind the server judges not expected with its word, an icon and the server's sentence", async () => {
    const sentence = "Profile lake-sr has no tag service, so its plugins are not expected to download tags.";
    serve(
      answers(
        ok(
          pluginResponse({
            kinds: [
              kind({ kind: "policies", state: "up_to_date" }),
              kind({ kind: "tags", state: "not_expected", lastDownloadAt: null, newestReportedVersion: null, sentence }),
            ],
          }),
        ),
      ),
    );
    open();
    const row = await within(await screen.findByRole("tabpanel", {}, T)).findByRole("row", { name: /Tags/ }, T);
    expect(within(row).getByText("Not expected")).toBeInTheDocument();
    expect(row.querySelector("svg")).not.toBeNull();
    expect(within(row).getByText(sentence)).toBeInTheDocument();
  });

  it("shows the last download as a <time> with its relative text, and the three versions as tabular numbers", async () => {
    serve(
      answers(
        ok(
          pluginResponse({
            kinds: [
              kind({ kind: "tags", state: "up_to_date", currentVersion: 4, newestReportedVersion: 3, servedVersion: 9 }),
              kind({ kind: "policies", state: "up_to_date", currentVersion: 12 }),
            ],
          }),
        ),
      ),
    );
    open();
    const p = await screen.findByRole("tabpanel", {}, T);
    const rows = await within(p).findAllByRole("row", { name: /^(Policies|Tags)/ }, T);
    // Policies first, then tags, whatever order the server answered in.
    expect(rows.map((r) => r.getAttribute("aria-label"))).toEqual(["Policies", "Tags"]);
    const tags = rows[1];
    const time = tags.querySelector("time");
    expect(time?.getAttribute("dateTime")).toBe("2026-10-19T09:58:00Z");
    expect(time?.textContent).toMatch(/ago|in /);
    for (const v of ["4", "3", "9"]) {
      const cell = within(tags).getByText(v);
      expect(cell.className).toContain("tabular-nums");
    }
  });

  it("states the rule a version is counted by", async () => {
    serve(answers(ok(pluginResponse({ kinds: [kind({ kind: "policies", state: "up_to_date" })] }))));
    open();
    expect(
      await within(await screen.findByRole("tabpanel", {}, T)).findByText(
        "A version counts as held once a plugin reports it; confirmation takes two polls, about a minute at the default 30 s poll.",
        {},
        T,
      ),
    ).toBeInTheDocument();
  });

  it("for an external engine shows the not-judged sentence and the raw rows, and no kind row", async () => {
    serve(
      answers(
        ok(
          pluginResponse({
            engine: { id: E_TR, name: "lake-trino", type: "trino", origin: "external", status: "running" },
            judged: {
              state: "not_judged",
              reason: "external",
              sentence: "egret does not judge external engines: it does not run them and cannot tell whether they should be polling.",
            },
            kinds: [],
            rows: [
              { pluginId: "trino@host-b-lake-tr", kind: "policies", lastKnownVersion: 6, servedVersion: 7, at: "2026-10-19T09:59:00Z" },
              { pluginId: "trino@host-a-lake-tr", kind: "tags", lastKnownVersion: -1, servedVersion: null, at: "2026-10-19T09:00:00Z" },
            ],
          }),
        ),
      ),
    );
    open();
    const p = await screen.findByRole("tabpanel", {}, T);
    expect(
      await within(p).findByText(
        "egret does not judge external engines: it does not run them and cannot tell whether they should be polling.",
        {},
        T,
      ),
    ).toBeInTheDocument();
    expect(within(p).queryByRole("row", { name: /^Policies$/ })).toBeNull();
    const raw = within(p).getByRole("table", { name: "Downloads reported by plugins" });
    expect(within(raw).getByText("trino@host-b-lake-tr")).toBeInTheDocument();
    expect(within(raw).getByText("trino@host-a-lake-tr")).toBeInTheDocument();
  });

  it("for a provisioned engine that is not running shows its sentence", async () => {
    serve(
      answers(
        ok(
          pluginResponse({
            judged: { state: "not_judged", reason: "not_running", sentence: "egret judges plugins only of running engines." },
          }),
        ),
      ),
    );
    open();
    expect(
      await within(await screen.findByRole("tabpanel", {}, T)).findByText("egret judges plugins only of running engines.", {}, T),
    ).toBeInTheDocument();
  });

  it("on a shared profile says the rows may come from any engine bound to it", async () => {
    serve(answers(ok(pluginResponse({ sharedWith: [{ id: E_SR, name: "lake-trino-2", origin: "provisioned" }] }))));
    open();
    expect(
      await within(await screen.findByRole("tabpanel", {}, T)).findByText(
        "These rows may come from any engine bound to lake-tr.",
        {},
        T,
      ),
    ).toBeInTheDocument();
  });

  it("offers Download Trino config on a Trino profile and opens the config dialog", async () => {
    serve(answers(ok(pluginResponse()), { "/policies/services/lake-tr/trino-config": ok({ files: [], warnings: [] }) }));
    open();
    const btn = await within(await screen.findByRole("tabpanel", {}, T)).findByRole("button", { name: "Download Trino config" }, T);
    await userEvent.click(btn);
    expect(await screen.findByRole("dialog", { name: /Trino config/ }, T)).toBeInTheDocument();
  });

  it("offers no Trino config on a StarRocks engine", async () => {
    const sr: EnginePluginResponse = pluginResponse({
      engine: { id: E_SR, name: "lake-sr", type: "starrocks", origin: "provisioned", status: "running" },
      profile: { serviceId: 3, name: "lake-sr", policyVersion: 2, tagVersion: 0, updatedAt: "2026-10-19T09:00:00Z" },
      kinds: [kind({ kind: "policies", state: "up_to_date" })],
    });
    serve({
      [`/perm/engines/${E_SR}/grants`]: ok(
        grantsResponse({
          engine: { id: E_SR, name: "lake-sr", type: "starrocks", origin: "provisioned", status: "running" },
          profile: { serviceId: 3, name: "lake-sr" },
          reach: { engines: [{ id: E_SR, name: "lake-sr", origin: "provisioned" }], sharedProfile: false },
        }),
      ),
      "/policies/services": ok([profileDto("lake-sr", { id: 3, type: "starrocks" })]),
      "/policies?service=lake-sr": ok([]),
      "/perm/overlaps": ok([]),
      [`/governance/engines/${E_SR}/plugin`]: ok(sr),
    });
    open(ENGINE_READER, E_SR);
    const p = await screen.findByRole("tabpanel", {}, T);
    await within(p).findByRole("row", { name: "Policies" }, T);
    expect(within(p).queryByRole("button", { name: "Download Trino config" })).toBeNull();
  });

  it("shows a skeleton while the plugin route answers", async () => {
    serve(answers("never"));
    open();
    const p = await screen.findByRole("tabpanel", {}, T);
    expect(within(p).getByTestId("plugin-loading")).toBeInTheDocument();
  });

  it("shows the error with Retry, which asks again", async () => {
    const served = serve(answers(fail(500)));
    open();
    const p = await screen.findByRole("tabpanel", {}, T);
    const retry = await within(p).findByRole("button", { name: "Retry" }, T);
    const before = served.asked.filter((a) => a.includes("/plugin")).length;
    await userEvent.click(retry);
    await vi.waitFor(() => expect(served.asked.filter((a) => a.includes("/plugin")).length).toBeGreaterThan(before));
    expect(panel()).toBeInTheDocument();
  });
});
