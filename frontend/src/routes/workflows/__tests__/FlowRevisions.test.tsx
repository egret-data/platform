import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

vi.mock("@/components/monaco/MonacoEditor", () => import("./monacoStub"));
vi.mock("@/components/workflows/editor/monacoYaml", () => ({ configureFlowYaml: vi.fn() }));
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import FlowDetail from "@/routes/workflows/FlowDetail";
import { FakeWorkflowServer, FLOW_ID, flowSource, WS } from "./fakeWorkflowServer";
import { resetMonacoStub, stub } from "./monacoStub";

/**
 * The Revisions tab (W7a Task 7), through the real flow page, its orval hooks
 * and the fake server (FE-TEST-1): restore is the page's save path, so the
 * page is what is under test.
 */

const FLOW_PATH = `/api/ws/${WS}/workflows/flows/${FLOW_ID}`;
const V1 = flowSource("first");
const V2 = flowSource("second");
const V3 = flowSource("third");

let server: FakeWorkflowServer;

beforeEach(() => {
  resetMonacoStub();
  toast.success.mockReset();
  toast.error.mockReset();
  toast.warning.mockReset();
  localStorage.clear();
});
afterEach(() => {
  vi.unstubAllGlobals();
});

function Where() {
  const location = useLocation();
  const navigate = useNavigate();
  return (
    <>
      <output data-testid="path">{location.pathname}</output>
      <output data-testid="search">{location.search}</output>
      <button type="button" onClick={() => navigate(-1)}>
        Browser back
      </button>
    </>
  );
}

function renderAt(tab = "revisions", search = "") {
  server.install();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/explore/${WS}/workflows/flows/${FLOW_ID}/${tab}${search}`]}>
        <Routes>
          <Route path="/explore/:workspace/workflows/flows/:id/:tab" element={<FlowDetail />} />
        </Routes>
        <Where />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/**
 * Versions v1..vn of `sources`, published in order (so the fake server lists
 * them OLDEST first, the opposite of the real one, and the page must sort),
 * then the saved draft set to `draft`.
 */
function flowWithVersions(sources: string[], draft: string, extra: Partial<{ can_manage: boolean }> = {}) {
  server = new FakeWorkflowServer({ source: sources[0], ...extra });
  sources.forEach((s, i) => {
    server.flow = { ...server.flow!, source: s };
    const v = server.publishNow(`note ${i + 1}`);
    v.created_by_name = i === 0 ? null : `Author ${i + 1}`;
  });
  server.flow = { ...server.flow!, source: draft };
}

const search = () => screen.getByTestId("search").textContent ?? "";
const puts = () => server.callsTo("PUT", FLOW_PATH);
const restoreButton = () => screen.findByRole("button", { name: "Restore to draft" });
const switchTab = (name: string) => fireEvent.mouseDown(screen.getByRole("tab", { name }), { button: 0 });
const lastDiff = () => stub.diffs[stub.diffs.length - 1];

describe("FlowRevisions: the list", () => {
  it("lists every version newest first with its note, author and time, and marks the latest", async () => {
    flowWithVersions([V1, V2, V3], V3);
    renderAt();
    const table = await screen.findByRole("table");
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows.map((r) => within(r).getByRole("link").textContent)).toEqual(["v3", "v2", "v1"]);
    expect(within(rows[0]).getByText("Latest")).toBeInTheDocument();
    expect(within(rows[1]).queryByText("Latest")).not.toBeInTheDocument();
    expect(within(rows[0]).getByText("note 3")).toBeInTheDocument();
    expect(within(rows[0]).getByText("Author 3")).toBeInTheDocument();
    // No author name: a dash, never an id or a member lookup.
    expect(within(rows[2]).getByText("—")).toBeInTheDocument();
    expect(server.calls.some((c) => c.path.includes("members"))).toBe(false);
    const created = new Date(server.versions[0].created_at).toLocaleString();
    expect(within(rows[2]).getByText(created)).toBeInTheDocument();
    // The saved draft is v3's text: content, not a timestamp.
    expect(within(rows[0]).getByText("Matches draft")).toBeInTheDocument();
    expect(within(rows[1]).queryByText("Matches draft")).not.toBeInTheDocument();
  });

  it("shows a skeleton while the versions load", async () => {
    flowWithVersions([V1], V1);
    let open!: () => void;
    server.versionsGate = new Promise((r) => { open = r; });
    renderAt();
    expect(await screen.findByTestId("list-skeleton")).toBeInTheDocument();
    expect(screen.queryByText("No versions yet")).not.toBeInTheDocument();
    open();
    expect(await screen.findByRole("link", { name: "v1" })).toBeInTheDocument();
  });

  it("says a failed read and retries, never showing it as no versions", async () => {
    flowWithVersions([V1], V1);
    server.versionsReply = { status: 503, body: { error: "the database is unavailable" } };
    renderAt();
    expect(await screen.findByText(/The versions could not be loaded: the database is unavailable/)).toBeInTheDocument();
    expect(screen.queryByText("No versions yet")).not.toBeInTheDocument();
    server.versionsReply = null;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("link", { name: "v1" })).toBeInTheDocument();
  });

  it("is empty with no version, and names the Create version control that exists", async () => {
    server = new FakeWorkflowServer();
    renderAt();
    expect(await screen.findByText("No versions yet")).toBeInTheDocument();
    expect(screen.getByText(/Create one with Create version on the Edit tab/)).toBeInTheDocument();
    // FE-COPY-3: the copy names a control; it is there under that name.
    expect(screen.getByRole("button", { name: "Create version" })).toBeInTheDocument();
  });
});

describe("FlowRevisions: selecting and comparing", () => {
  it("shows the latest version's YAML, and a selected version's, with ?v= in the URL and Back walking it", async () => {
    flowWithVersions([V1, V2], V2);
    renderAt();
    expect(await screen.findByRole("textbox", { name: "v2 source" })).toHaveValue(V2);
    await userEvent.click(screen.getByRole("link", { name: "v1" }));
    expect(search()).toBe("?v=1");
    expect(await screen.findByRole("textbox", { name: "v1 source" })).toHaveValue(V1);
    expect(screen.getByRole("link", { name: "v1" })).toHaveAttribute("aria-current", "true");
    await userEvent.click(screen.getByRole("button", { name: "Browser back" }));
    expect(search()).toBe("");
    expect(await screen.findByRole("textbox", { name: "v2 source" })).toHaveValue(V2);
  });

  it("compares with the current draft: the diff gets the version on the left and the saved draft on the right", async () => {
    const draft = flowSource("draft");
    flowWithVersions([V1, V2], draft);
    renderAt("revisions", "?v=1");
    await screen.findByRole("textbox", { name: "v1 source" });
    await userEvent.click(screen.getByRole("combobox", { name: "Compare with" }));
    await userEvent.click(await screen.findByRole("option", { name: "Current draft" }));
    expect(new URLSearchParams(search()).get("compare")).toBe("draft");
    expect(new URLSearchParams(search()).get("v")).toBe("1");
    expect(await screen.findByRole("textbox", { name: "Original" })).toHaveValue(V1);
    expect(screen.getByRole("textbox", { name: "Modified" })).toHaveValue(draft);
    expect(lastDiff()).toMatchObject({ original: V1, modified: draft, language: "yaml" });
    expect(screen.getByTestId("revision-diff-labels")).toHaveTextContent(/^v1Current draft$/);
  });

  it("compares two versions with the older on the left, whichever is selected", async () => {
    flowWithVersions([V1, V2, V3], V3);
    renderAt("revisions", "?v=3&compare=1");
    expect(await screen.findByRole("textbox", { name: "Original" })).toHaveValue(V1);
    expect(screen.getByRole("textbox", { name: "Modified" })).toHaveValue(V3);
    // Selecting the compare target itself drops the comparison rather than diffing a version with itself.
    await userEvent.click(screen.getByRole("link", { name: "v1" }));
    expect(search()).toBe("?v=1");
    expect(await screen.findByRole("textbox", { name: "v1 source" })).toHaveValue(V1);
    await userEvent.click(screen.getByRole("combobox", { name: "Compare with" }));
    expect(screen.queryByRole("option", { name: "v1" })).not.toBeInTheDocument();
    await userEvent.click(await screen.findByRole("option", { name: "v2" }));
    expect(screen.getByRole("textbox", { name: "Original" })).toHaveValue(V1);
    expect(screen.getByRole("textbox", { name: "Modified" })).toHaveValue(V2);
    // "Nothing" goes back to one version's YAML.
    await userEvent.click(screen.getByRole("combobox", { name: "Compare with" }));
    await userEvent.click(await screen.findByRole("option", { name: "Nothing" }));
    expect(search()).toBe("?v=1");
    expect(await screen.findByRole("textbox", { name: "v1 source" })).toBeInTheDocument();
  });

  it("says when ?v= names no version, and shows the latest", async () => {
    flowWithVersions([V1, V2], V2);
    renderAt("revisions", "?v=9");
    expect(await screen.findByText("There is no version 9; showing v2, the latest.")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "v2 source" })).toHaveValue(V2);
  });
});

describe("FlowRevisions: restore to draft", () => {
  it("asks first, saying it changes what runs, then saves the version's text with if_updated_at, never publishes, toasts and refetches", async () => {
    flowWithVersions([V1, V2], V2);
    const updatedAt = server.flow!.updated_at;
    renderAt("revisions", "?v=1");
    await screen.findByRole("textbox", { name: "v1 source" });
    const versionReads = server.callsTo("GET", `${FLOW_PATH}/versions`).length;
    await userEvent.click(await restoreButton());
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/the next run, schedule, webhook or subflow call runs v1/)).toBeInTheDocument();
    // The draft is v2's text, which v2 keeps; no unsaved edits: nothing to lose.
    expect(within(dialog).queryByTestId("restore-loses-draft")).not.toBeInTheDocument();
    expect(within(dialog).queryByTestId("restore-loses-edits")).not.toBeInTheDocument();
    expect(puts()).toHaveLength(0);

    await userEvent.click(within(dialog).getByRole("button", { name: "Restore v1" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    // Verbatim: the microseconds a `Date` would drop are still there, as a string.
    expect(puts()[0].body).toEqual({ source: V1, folder_id: null, if_updated_at: updatedAt });
    expect(server.callsTo("POST", `${FLOW_PATH}/publish`)).toHaveLength(0);
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Restored v1 to the draft. New runs use it."));
    await waitFor(() => expect(server.callsTo("GET", `${FLOW_PATH}/versions`).length).toBeGreaterThan(versionReads));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(server.flow!.source).toBe(V1);
    // It stays on the tab; the draft now matches v1, so Restore says why it is off.
    expect(screen.getByTestId("path")).toHaveTextContent(`/flows/${FLOW_ID}/revisions`);
    const v1Row = screen.getByRole("link", { name: "v1" }).closest("tr")!;
    await waitFor(() => expect(within(v1Row).getByText("Matches draft")).toBeInTheDocument());
    expect(await restoreButton()).toBeDisabled();
    expect(screen.getByText("The saved draft already has this text")).toBeInTheDocument();
    // The editor holds the restored text, saved.
    switchTab("Edit");
    expect(await screen.findByRole("textbox", { name: "Flow source" })).toHaveValue(V1);
    expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument();
  });

  it("names what is lost: a saved draft no version keeps, and the editor's unsaved edits", async () => {
    flowWithVersions([V1, V2], flowSource("saved, never versioned"));
    renderAt("edit");
    const box = await screen.findByRole("textbox", { name: "Flow source" });
    fireEvent.change(box, { target: { value: flowSource("typed, never saved") } });
    switchTab("Revisions");
    await userEvent.click(await screen.findByRole("link", { name: "v1" }));
    await userEvent.click(await restoreButton());
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByTestId("restore-loses-draft")).toHaveTextContent(
      "The saved draft has changes since v2 that no version keeps; restoring discards them.",
    );
    expect(within(dialog).getByTestId("restore-loses-edits")).toHaveTextContent("Your unsaved edits in the editor are discarded too.");
    // The copy names Create version; it exists under that name (FE-COPY-3).
    expect(within(dialog).getByText(/use Create version first/)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Create version", hidden: true }).length).toBeGreaterThan(0);
    // Cancel writes nothing and keeps the edits.
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(puts()).toHaveLength(0);
    switchTab("Edit");
    expect(await screen.findByRole("textbox", { name: "Flow source" })).toHaveValue(flowSource("typed, never saved"));
  });

  it("does not claim a loss when an older version keeps the saved draft", async () => {
    flowWithVersions([V1, V2], V1);
    renderAt("revisions", "?v=2");
    await userEvent.click(await restoreButton());
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).queryByTestId("restore-loses-draft")).not.toBeInTheDocument();
  });

  it("on a changed draft goes through the stale-save dialog, and Overwrite saves the version's text unconditionally", async () => {
    flowWithVersions([V1, V2], V2);
    renderAt("edit");
    const box = await screen.findByRole("textbox", { name: "Flow source" });
    fireEvent.change(box, { target: { value: flowSource("mine") } });
    switchTab("Revisions");
    await userEvent.click(await screen.findByRole("link", { name: "v1" }));
    server.saveElsewhere(flowSource("theirs"));
    await userEvent.click(await restoreButton());
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Restore v1" }));
    const stale = await screen.findByRole("alertdialog", { name: "The draft changed since you opened it" });
    expect(server.flow!.source).toBe(flowSource("theirs"));
    // Nothing was written; the editor still holds the user's text.
    await userEvent.click(within(stale).getByRole("button", { name: "Cancel" }));
    switchTab("Edit");
    expect(await screen.findByRole("textbox", { name: "Flow source" })).toHaveValue(flowSource("mine"));

    switchTab("Revisions");
    // The tab switch is a new URL without `?v=`: the latest, v2, is selected again.
    await userEvent.click(await screen.findByRole("link", { name: "v1" }));
    await userEvent.click(await restoreButton());
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Restore v1" }));
    const again = await screen.findByRole("alertdialog", { name: "The draft changed since you opened it" });
    await userEvent.click(within(again).getByRole("button", { name: "Overwrite" }));
    await waitFor(() => expect(server.flow!.source).toBe(V1));
    expect(puts().at(-1)!.body).toEqual({ source: V1, folder_id: null });
    expect(server.callsTo("POST", `${FLOW_PATH}/publish`)).toHaveLength(0);
    switchTab("Edit");
    expect(await screen.findByRole("textbox", { name: "Flow source" })).toHaveValue(V1);
  });

  it("on failure shows the server's message and leaves the editor's text alone", async () => {
    flowWithVersions([V1, V2], V2);
    renderAt("edit");
    const box = await screen.findByRole("textbox", { name: "Flow source" });
    fireEvent.change(box, { target: { value: flowSource("mine") } });
    switchTab("Revisions");
    await userEvent.click(await screen.findByRole("link", { name: "v1" }));
    server.putReply = { status: 500, body: { error: "the database is unavailable" } };
    await userEvent.click(await restoreButton());
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Restore v1" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith(expect.stringContaining("the database is unavailable")));
    expect(toast.success).not.toHaveBeenCalled();
    switchTab("Edit");
    expect(await screen.findByRole("textbox", { name: "Flow source" })).toHaveValue(flowSource("mine"));
  });

  it("is off, with the reason shown, without workflow:manage", async () => {
    flowWithVersions([V1, V2], V2, { can_manage: false });
    renderAt("revisions", "?v=1");
    expect(await restoreButton()).toBeDisabled();
    expect(screen.getByText("Restoring needs the workflow:manage permission")).toBeInTheDocument();
  });
});
