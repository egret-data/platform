import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

vi.mock("@/components/monaco/MonacoEditor", () => import("./monacoStub"));
vi.mock("@/components/workflows/editor/monacoYaml", () => ({ configureFlowYaml: vi.fn() }));
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import FlowDetail from "@/routes/workflows/FlowDetail";
import FlowNew from "@/routes/workflows/FlowNew";
import { LAYOUT_KEY_PREFIX } from "@/components/workflows/panels/usePersistedLayout";
import type { Layout } from "@/components/workflows/panels/layout";
import { serveFiles, type FilesRequest, type FilesStore, type FilesWorld } from "@/components/workflows/namespaces/files/__tests__/fakeFilesServer";
import { FakeWorkflowServer, FLOW_ID, flowSource, WS } from "./fakeWorkflowServer";
import { resetMonacoStub, stub } from "./monacoStub";
import { FLOW_CODE_CONTEXT } from "@/components/workflows/editor/CodeTab";
import { NS_FILE_CONTEXT } from "@/components/workflows/namespaces/files/NamespaceFileEditor";

/**
 * THE FLOW EDITOR'S FILES (W7d spec §4.3, "In the flow editor"): the Files
 * tab, and one `file:<path>` tab per open namespace file, through the real
 * flow page, its real frame and Task 6's real explorer. The flow routes and
 * the namespace file routes are both served from memory.
 */
const NS = "company.team";
const FLOW_PATH = `/api/ws/${WS}/workflows/flows/${FLOW_ID}`;
const LAYOUT_KEY = `${LAYOUT_KEY_PREFIX}${FLOW_ID}`;
const LONG = { timeout: 3000 };

const FILES: FilesWorld["files"] = {
  "/scripts/b.py": "b = 2\n",
  "/a.py": "a = 1\n",
  "/README.md": "# Team\n",
};

let server: FakeWorkflowServer;
let requests: FilesRequest[] = [];
let store: FilesStore;
/** Every request, `METHOD url`, flow and file routes alike, in the order sent. */
let fetched: string[] = [];

beforeEach(() => {
  resetMonacoStub();
  for (const f of Object.values(toast)) f.mockReset();
  localStorage.clear();
});
afterEach(() => vi.unstubAllGlobals());

function storeLayout(layout: Layout) {
  localStorage.setItem(LAYOUT_KEY, JSON.stringify(layout));
}

/** Code on the left; Files and the given file tabs on the right, the last one selected. */
function layoutWith(...fileTabs: string[]): Layout {
  return {
    panels: [
      { id: "p1", size: 60, tabs: ["code"], active: "code" },
      { id: "p2", size: 40, tabs: ["files", ...fileTabs], active: fileTabs.at(-1) ?? "files" },
    ],
  };
}

function renderFlow({ canManage = true, world = {} }: { canManage?: boolean; world?: Partial<FilesWorld> } = {}) {
  server = new FakeWorkflowServer({ can_manage: canManage });
  ({ requests, store } = serveFiles({ namespace: NS, files: FILES, ...world }));
  const filesFetch = globalThis.fetch;
  server.install();
  const flowFetch = globalThis.fetch;
  fetched = [];
  vi.stubGlobal("fetch", (input: string, init?: RequestInit) => {
    fetched.push(`${(init?.method ?? "GET").toUpperCase()} ${String(input)}`);
    return String(input).includes("/workflows/namespaces/") ? filesFetch(input, init) : flowFetch(input, init);
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false }, mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[`/explore/${WS}/workflows/flows/${FLOW_ID}/edit`]}>
          <Routes>
            <Route path="/explore/:workspace/workflows/flows/:id/:tab" element={<FlowDetail />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

const strip = (n: number) => screen.getByRole("tablist", { name: `Panel ${n}` });
const tabNames = (n: number) => within(strip(n)).getAllByRole("tab").map((t) => t.getAttribute("aria-label") ?? t.textContent);
const tree = () => screen.findByRole("tree", { name: `Files of ${NS}` });
const row = async (name: string) => within(await tree()).findByRole("treeitem", { name });
const fileBox = (name: string) => screen.findByRole("textbox", { name: `Contents of ${name}` }, LONG);
const flowBox = () => screen.findByRole("textbox", { name: "Flow source" });
const type = (box: HTMLElement, text: string) => fireEvent.change(box, { target: { value: text } });
const ctrlS = (el: HTMLElement) => fireEvent.keyDown(el, { key: "s", ctrlKey: true });
const uploads = () => requests.filter((r) => r.method === "POST" && r.route === "");
const puts = () => server.callsTo("PUT", FLOW_PATH);
const panelOf = (tabName: string | RegExp) => screen.getByRole("tabpanel", { name: tabName });

/**
 * Menus open from the keyboard: in jsdom every element has a zero rect, so the panels' resize
 * handler takes a pointer press anywhere for a gutter grab and cancels it (`MultiPanel.test.tsx`).
 */
async function openMenu(user: ReturnType<typeof userEvent.setup>, button: HTMLElement) {
  button.focus();
  await user.keyboard("{Enter}");
}

async function openFromPlus(user: ReturnType<typeof userEvent.setup>, panel: number, item: string) {
  screen.getByRole("button", { name: `Reopen a tab in panel ${panel}` }).focus();
  await user.keyboard("{Enter}");
  await user.click(await screen.findByRole("menuitem", { name: item }));
}

describe("the Files tab", () => {
  it("is a tab of the editor that shows the flow's namespace tree", async () => {
    const user = userEvent.setup();
    renderFlow();
    await flowBox();
    await openFromPlus(user, 2, "Files");
    expect(await row("a.py")).toBeInTheDocument();
    expect(await row("scripts")).toBeInTheDocument();
    expect(tabNames(2)).toContain("Files");
  });

  it("opens a file as a tab in the panel holding Code, and focuses that tab when opened again", async () => {
    const user = userEvent.setup();
    renderFlow();
    await flowBox();
    await openFromPlus(user, 2, "Files");
    await user.click(await row("a.py"));
    await waitFor(() => expect(tabNames(1)).toEqual(["Code", "a.py (/a.py)"]));
    expect(within(strip(1)).getByRole("tab", { name: "a.py (/a.py)" })).toHaveAttribute("aria-selected", "true");
    expect(await fileBox("a.py")).toHaveValue("a = 1\n");

    await user.click(within(strip(1)).getByRole("tab", { name: "Code" }));
    await user.click(await row("a.py"));
    expect(tabNames(1)).toEqual(["Code", "a.py (/a.py)"]);
    expect(within(strip(1)).getByRole("tab", { name: "a.py (/a.py)" })).toHaveAttribute("aria-selected", "true");
  });

  it("titles a file tab with the file's name and names it with its path", async () => {
    storeLayout(layoutWith("file:scripts/b.py"));
    renderFlow();
    const tab = await within(await screen.findByRole("tablist", { name: "Panel 2" })).findByRole("tab", { name: "b.py (/scripts/b.py)" });
    expect(tab).toHaveTextContent(/^b\.py$/);
  });

  it("keeps the file's text when another tab is shown and it comes back", async () => {
    const user = userEvent.setup();
    storeLayout(layoutWith("file:a.py", "file:scripts/b.py"));
    renderFlow();
    await fileBox("b.py");
    await user.click(within(strip(2)).getByRole("tab", { name: "a.py (/a.py)" }));
    type(await fileBox("a.py"), "a = 42\n");
    await user.click(within(strip(2)).getByRole("tab", { name: "b.py (/scripts/b.py)" }));
    await fileBox("b.py");
    await user.click(within(strip(2)).getByRole("tab", { name: "a.py (/a.py), unsaved" }));
    expect(await fileBox("a.py")).toHaveValue("a = 42\n");
  });
});

describe("saving files apart from the flow", () => {
  it("shows a dot and says 'unsaved' on an edited file's tab", async () => {
    storeLayout(layoutWith("file:a.py"));
    renderFlow();
    type(await fileBox("a.py"), "a = 2\n");
    expect(await within(strip(2)).findByRole("tab", { name: "a.py (/a.py), unsaved" })).toBeInTheDocument();
    expect(screen.getByText("1 unsaved file")).toBeInTheDocument();
  });

  it("Ctrl/Cmd+S in a file tab saves that file only, with its revision, and not the flow", async () => {
    storeLayout(layoutWith("file:a.py"));
    renderFlow();
    type(await flowBox(), flowSource("edited"));
    type(await fileBox("a.py"), "a = 2\n");
    ctrlS(await fileBox("a.py"));
    await waitFor(() => expect(uploads()).toHaveLength(1));
    expect(uploads()[0].query.get("path")).toBe("/a.py");
    expect(uploads()[0].query.get("if_revision")).toBe("1");
    expect(store.text("/a.py")).toBe("a = 2\n");
    await waitFor(() => expect(within(strip(2)).getByRole("tab", { name: "a.py (/a.py)" })).toBeInTheDocument());
    // The flow's own edit is still unsaved, and was never sent.
    expect(puts()).toHaveLength(0);
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
  });

  it("the flow's Save saves the flow only", async () => {
    const user = userEvent.setup();
    storeLayout(layoutWith("file:a.py"));
    renderFlow();
    type(await fileBox("a.py"), "a = 2\n");
    type(await flowBox(), flowSource("edited"));
    await user.click(within(screen.getByRole("banner")).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(uploads()).toHaveLength(0);
    expect(within(strip(2)).getByRole("tab", { name: "a.py (/a.py), unsaved" })).toBeInTheDocument();
  });

  it("Save all files saves every edited file, shown or not, and not the flow", async () => {
    const user = userEvent.setup();
    storeLayout(layoutWith("file:a.py", "file:README.md", "file:scripts/b.py"));
    renderFlow();
    type(await flowBox(), flowSource("edited"));
    type(await fileBox("b.py"), "b = 3\n");
    await user.click(within(strip(2)).getByRole("tab", { name: "a.py (/a.py)" }));
    type(await fileBox("a.py"), "a = 3\n");
    const saveAll = screen.getByRole("button", { name: "Save all files" });
    expect(saveAll).toBeEnabled();
    await user.click(saveAll);
    await waitFor(() => expect(uploads()).toHaveLength(2));
    expect(uploads().map((u) => u.query.get("path")).sort()).toEqual(["/a.py", "/scripts/b.py"]);
    expect(store.text("/scripts/b.py")).toBe("b = 3\n");
    expect(store.text("/a.py")).toBe("a = 3\n");
    expect(puts()).toHaveLength(0);
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Saved 2 files."));
    await waitFor(() => expect(screen.getByRole("button", { name: "Save all files" })).toBeDisabled());
    expect(screen.getByText("No unsaved files")).toBeInTheDocument();
  });

  it("a 409 file_changed during Save all opens the stale dialog for that file, even when its tab is not shown", async () => {
    const user = userEvent.setup();
    storeLayout(layoutWith("file:a.py", "file:scripts/b.py"));
    renderFlow();
    type(await fileBox("b.py"), "b = 3\n");
    await user.click(within(strip(2)).getByRole("tab", { name: "a.py (/a.py)" }));
    type(await fileBox("a.py"), "a = 3\n");
    await user.click(within(strip(2)).getByRole("tab", { name: "b.py (/scripts/b.py), unsaved" }));
    await fileBox("b.py");
    // Someone else saves a.py meanwhile.
    store.write("/a.py", new TextEncoder().encode("a = 'theirs'\n"));
    await user.click(screen.getByRole("button", { name: "Save all files" }));
    const dialog = await screen.findByRole("alertdialog", { name: "a.py changed since you opened it" }, LONG);
    expect(dialog).toHaveTextContent("it is at revision 2, not 1");
    expect(store.text("/scripts/b.py")).toBe("b = 3\n");
    await user.click(within(dialog).getByRole("button", { name: "Overwrite" }));
    await waitFor(() => expect(store.text("/a.py")).toBe("a = 3\n"));
    expect(uploads().at(-1)!.query.get("if_revision")).toBeNull();
  });

  it("warns before the page is left while a file has unsaved edits", async () => {
    storeLayout(layoutWith("file:a.py"));
    renderFlow();
    const leave = () => {
      const e = new Event("beforeunload", { cancelable: true });
      window.dispatchEvent(e);
      return e.defaultPrevented;
    };
    await fileBox("a.py");
    expect(leave()).toBe(false);
    type(await fileBox("a.py"), "a = 2\n");
    await waitFor(() => expect(leave()).toBe(true));
  });

  it("asks before closing a file tab with unsaved edits", async () => {
    const user = userEvent.setup();
    storeLayout(layoutWith("file:a.py"));
    renderFlow();
    type(await fileBox("a.py"), "a = 2\n");
    const tab = await within(strip(2)).findByRole("tab", { name: "a.py (/a.py), unsaved" });
    tab.focus();
    await user.keyboard("{Delete}");
    const ask = await screen.findByRole("alertdialog", { name: "Close a.py without saving?" });
    await user.click(within(ask).getByRole("button", { name: "Keep editing" }));
    expect(within(strip(2)).getByRole("tab", { name: "a.py (/a.py), unsaved" })).toBeInTheDocument();

    within(strip(2)).getByRole("tab", { name: "a.py (/a.py), unsaved" }).focus();
    await user.keyboard("{Delete}");
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Discard and close" }));
    await waitFor(() => expect(within(strip(2)).queryByRole("tab", { name: /a\.py/ })).toBeNull());
    expect(uploads()).toHaveLength(0);
  });
});

describe("file tabs in the stored layout", () => {
  it("restores a file tab and reads its content again", async () => {
    storeLayout(layoutWith("file:scripts/b.py"));
    renderFlow();
    expect(await fileBox("b.py")).toHaveValue("b = 2\n");
    const stats = requests.filter((r) => r.route === "/stats");
    expect(stats.map((r) => r.query.get("path"))).toContain("/scripts/b.py");
  });

  it("shows a clear state for a file deleted meanwhile, and closes its tab from there", async () => {
    const user = userEvent.setup();
    storeLayout(layoutWith("file:gone.py"));
    renderFlow();
    const panel = await waitFor(() => panelOf("gone.py (/gone.py)"));
    expect(await within(panel).findByText("/gone.py is not in company.team any more", {}, LONG)).toBeInTheDocument();
    await user.click(within(panel).getByRole("button", { name: "Close this tab" }));
    await waitFor(() => expect(within(strip(2)).queryByRole("tab", { name: /gone\.py/ })).toBeNull());
  });

  it("a file deleted after it opened: its save opens the stale dialog that says so", async () => {
    storeLayout(layoutWith("file:a.py"));
    renderFlow();
    type(await fileBox("a.py"), "a = 2\n");
    const revs = store.files.get("/a.py")!;
    revs[revs.length - 1].deleted = true;
    ctrlS(await fileBox("a.py"));
    const dialog = await screen.findByRole("alertdialog", { name: "a.py changed since you opened it" }, LONG);
    expect(dialog).toHaveTextContent("was deleted");
  });
});

describe("the tree and the open tabs", () => {
  it("renaming a folder moves its open file tabs, and the stored layout follows", async () => {
    const user = userEvent.setup();
    storeLayout(layoutWith("file:a.py", "file:scripts/b.py"));
    renderFlow();
    await fileBox("b.py");
    await user.click(within(strip(2)).getByRole("tab", { name: "Files" }));
    const folder = await row("scripts");
    await openMenu(user, within(folder).getByRole("button", { name: "Actions for scripts" }));
    await user.click(await screen.findByRole("menuitem", { name: "Rename" }));
    const dialog = await screen.findByRole("dialog", { name: "Rename scripts" });
    // Set at once: typed key by key, a busy machine lost keys to the dialog's own focus handling.
    fireEvent.change(within(dialog).getByLabelText("New name"), { target: { value: "tools" } });
    await user.click(within(dialog).getByRole("button", { name: "Rename" }));
    await waitFor(() => expect(tabNames(2)).toEqual(["Files", "a.py (/a.py)", "b.py (/tools/b.py)"]));
    await waitFor(() => {
      const stored = JSON.parse(localStorage.getItem(LAYOUT_KEY) ?? "{}") as Layout;
      expect(stored.panels.flatMap((p) => p.tabs)).toEqual(["code", "files", "file:a.py", "file:tools/b.py"]);
    }, LONG);
    await user.click(within(strip(2)).getByRole("tab", { name: "b.py (/tools/b.py)" }));
    expect(await fileBox("b.py")).toHaveValue("b = 2\n");
  });

  it("renaming an open file renames its tab, which reads the file at its new path", async () => {
    const user = userEvent.setup();
    storeLayout(layoutWith("file:a.py", "files"));
    renderFlow();
    const file = await row("a.py");
    await openMenu(user, within(file).getByRole("button", { name: "Actions for a.py" }));
    await user.click(await screen.findByRole("menuitem", { name: "Rename" }));
    const dialog = await screen.findByRole("dialog", { name: "Rename a.py" });
    // Set at once: typed key by key, a busy machine lost keys to the dialog's own focus handling.
    fireEvent.change(within(dialog).getByLabelText("New name"), { target: { value: "c.py" } });
    await user.click(within(dialog).getByRole("button", { name: "Rename" }));
    await waitFor(() => expect(tabNames(2)).toEqual(["Files", "c.py (/c.py)"]));
    await user.click(within(strip(2)).getByRole("tab", { name: "c.py (/c.py)" }));
    expect(await fileBox("c.py")).toHaveValue("a = 1\n");
  });

  it("refuses to move a file whose tab has unsaved edits", async () => {
    const user = userEvent.setup();
    storeLayout(layoutWith("file:a.py", "files"));
    renderFlow();
    await user.click(within(await screen.findByRole("tablist", { name: "Panel 2" })).getByRole("tab", { name: "a.py (/a.py)" }));
    type(await fileBox("a.py"), "a = 2\n");
    await user.click(within(strip(2)).getByRole("tab", { name: "Files" }));
    const file = await row("a.py");
    await openMenu(user, within(file).getByRole("button", { name: "Actions for a.py" }));
    await user.click(await screen.findByRole("menuitem", { name: "Rename" }));
    const dialog = await screen.findByRole("dialog", { name: "Rename a.py" });
    // Set at once: typed key by key, a busy machine lost keys to the dialog's own focus handling.
    fireEvent.change(within(dialog).getByLabelText("New name"), { target: { value: "c.py" } });
    await user.click(within(dialog).getByRole("button", { name: "Rename" }));
    expect(await within(dialog).findByText("a.py has unsaved edits. Save or close it first.")).toBeInTheDocument();
    expect(requests.filter((r) => r.method === "PUT")).toHaveLength(0);
  });

  it("deleting an open file from the tree closes its tab", async () => {
    const user = userEvent.setup();
    storeLayout(layoutWith("file:a.py", "files"));
    renderFlow();
    await within(await screen.findByRole("tablist", { name: "Panel 2" })).findByRole("tab", { name: "a.py (/a.py)" });
    const file = await row("a.py");
    await openMenu(user, within(file).getByRole("button", { name: "Actions for a.py" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    await user.click(within(await screen.findByRole("alertdialog", { name: "Delete a.py?" })).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(tabNames(2)).toEqual(["Files"]));
  });
});

describe("a read-only viewer", () => {
  it("opens files read-only, cannot save them, and has no Save all", async () => {
    storeLayout(layoutWith("file:a.py"));
    renderFlow({ canManage: false });
    const box = await fileBox("a.py");
    expect(box).toHaveAttribute("readonly");
    const panel = panelOf("a.py (/a.py)");
    const save = within(panel).getByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    expect(save).toHaveAccessibleDescription("Changing namespace files needs the workflow:manage permission");
    ctrlS(box);
    await act(async () => { await new Promise((r) => setTimeout(r, 20)); });
    expect(uploads()).toHaveLength(0);
    expect(screen.queryByRole("button", { name: "Save all files" })).toBeNull();
  });

  it("sees the Files tree with its writes disabled", async () => {
    const user = userEvent.setup();
    renderFlow({ canManage: false });
    await flowBox();
    await openFromPlus(user, 2, "Files");
    await row("a.py");
    expect(screen.getByRole("button", { name: "New file" })).toBeDisabled();
  });
});

describe("the create page", () => {
  it("has a Files tab that says files open once the flow exists", async () => {
    const user = userEvent.setup();
    server = new FakeWorkflowServer(null);
    server.install();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <TooltipProvider>
          <MemoryRouter initialEntries={[`/explore/${WS}/workflows/flows/new`]}>
            <Routes>
              <Route path="/explore/:workspace/workflows/flows/new" element={<FlowNew />} />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>,
    );
    await flowBox();
    await openFromPlus(user, 2, "Files");
    expect(await screen.findByText("Files open once the flow is created")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save all files" })).toBeNull();
  });
});

describe("Ctrl/Cmd+S inside each editor (fix round 1)", () => {
  /**
   * Standalone Monaco keeps every editor's `addCommand` in one shared keybinding service, so a
   * command without a `when` answers in any editor (monaco-editor #2947). jsdom cannot run
   * Monaco's keybinding service; what it can check is that each editor scopes its command to a
   * context key only it carries, and that each command saves its own thing. The real-Monaco
   * proof (Code and a file side by side) is Task 9's browser run.
   */
  it("Code's command and a file editor's command are each scoped to their own editor's context key", async () => {
    storeLayout(layoutWith("file:a.py"));
    renderFlow();
    await flowBox();
    await fileBox("a.py");
    const saveKey = 2048 | 49; // KeyMod.CtrlCmd | KeyCode.KeyS, as the stub defines them
    const code = stub.bindings.filter((b) => b.keybinding === saveKey && b.context === FLOW_CODE_CONTEXT);
    const file = stub.bindings.filter((b) => b.keybinding === saveKey && b.context === NS_FILE_CONTEXT);
    expect(code).toHaveLength(1);
    expect(file).toHaveLength(1);
    // No save command answers in every editor.
    expect(stub.bindings.filter((b) => b.keybinding === saveKey && b.context === undefined)).toEqual([]);
    // Each key lives on the editor whose command names it.
    expect(stub.contextKeys.find((k) => k.key === FLOW_CODE_CONTEXT)?.path).toBe(code[0].path);
    expect(stub.contextKeys.find((k) => k.key === NS_FILE_CONTEXT)?.path).toBe(file[0].path);
    expect(code[0].path).not.toBe(file[0].path);
  });

  it("the file editor's command saves that file only; Code's command saves the flow only", async () => {
    storeLayout(layoutWith("file:a.py"));
    renderFlow();
    type(await flowBox(), flowSource("edited"));
    type(await fileBox("a.py"), "a = 2\n");
    const fileSave = stub.bindings.find((b) => b.context === NS_FILE_CONTEXT)!;
    act(() => fileSave.run());
    await waitFor(() => expect(uploads()).toHaveLength(1));
    expect(puts()).toHaveLength(0);
    const codeSave = stub.bindings.find((b) => b.context === FLOW_CODE_CONTEXT)!;
    act(() => codeSave.run());
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(uploads()).toHaveLength(1);
  });
});

describe("a flow save that changes the namespace (fix round 1)", () => {
  const OTHER = "company.other";
  const moved = (message: string) => flowSource(message).replace("namespace: company.team", `namespace: ${OTHER}`);
  const otherNamespaceFetches = () => fetched.filter((u) => u.includes(`/namespaces/${OTHER}/`));

  async function editFileAndMoveFlow() {
    storeLayout(layoutWith("file:a.py"));
    renderFlow();
    type(await fileBox("a.py"), "a = 2\n");
    await within(strip(2)).findByRole("tab", { name: "a.py (/a.py), unsaved" });
    type(await flowBox(), moved("moved"));
  }

  it("asks first when open files have unsaved edits; Save files first saves them into the old namespace, then the flow", async () => {
    const user = userEvent.setup();
    await editFileAndMoveFlow();
    await user.click(within(screen.getByRole("banner")).getByRole("button", { name: "Save" }));
    const ask = await screen.findByRole("alertdialog", { name: `Save the open files before the flow moves to ${OTHER}?` });
    expect(ask).toHaveTextContent("a.py has unsaved edits");
    expect(puts()).toHaveLength(0);
    await user.click(within(ask).getByRole("button", { name: "Save files first" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    // The file went to the namespace it was read from, before the flow left it.
    expect(uploads()).toHaveLength(1);
    expect(uploads()[0].path).toContain(`/namespaces/${NS}/files`);
    expect(store.text("/a.py")).toBe("a = 2\n");
    const fileWrite = fetched.findIndex((r) => r.startsWith("POST ") && r.includes(`/namespaces/${NS}/files?`));
    const flowWrite = fetched.findIndex((r) => r.startsWith("PUT ") && r.includes(FLOW_PATH));
    expect(fileWrite).toBeGreaterThan(-1);
    expect(fileWrite).toBeLessThan(flowWrite);
    // The tabs belonged to the old namespace: closed, never re-pointed.
    await waitFor(() => expect(tabNames(2)).toEqual(["Files"]));
    expect(otherNamespaceFetches().filter((u) => u.includes("/files/stats") || /\/files\?/.test(u))).toEqual([]);
  });

  it("Save files first keeps the flow where it is when a file is refused, and asks that file's question", async () => {
    const user = userEvent.setup();
    await editFileAndMoveFlow();
    // Someone else saves a.py meanwhile: its save is refused with 409 file_changed.
    store.write("/a.py", new TextEncoder().encode("a = 'theirs'\n"));
    await user.click(within(screen.getByRole("banner")).getByRole("button", { name: "Save" }));
    const ask = await screen.findByRole("alertdialog", { name: `Save the open files before the flow moves to ${OTHER}?` });
    await user.click(within(ask).getByRole("button", { name: "Save files first" }));
    expect(await screen.findByRole("alertdialog", { name: "a.py changed since you opened it" }, LONG)).toBeInTheDocument();
    expect(puts()).toHaveLength(0);
    // Behind the modal question, the tab is still open with its edits.
    expect(screen.getByRole("tab", { name: "a.py (/a.py), unsaved", hidden: true })).toBeInTheDocument();
  });

  it("Discard file changes saves the flow, sends no file, and closes the file tabs", async () => {
    const user = userEvent.setup();
    await editFileAndMoveFlow();
    await user.click(within(screen.getByRole("banner")).getByRole("button", { name: "Save" }));
    const ask = await screen.findByRole("alertdialog", { name: `Save the open files before the flow moves to ${OTHER}?` });
    await user.click(within(ask).getByRole("button", { name: "Discard file changes" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(uploads()).toHaveLength(0);
    expect(store.text("/a.py")).toBe("a = 1\n");
    await waitFor(() => expect(tabNames(2)).toEqual(["Files"]));
    await waitFor(() => expect(toast.info).toHaveBeenCalledWith(
      `The flow is now in ${OTHER}, so the file tabs of ${NS} were closed. Unsaved edits to a.py were discarded.`,
    ));
  });

  it("Cancel saves nothing and keeps the file's edits", async () => {
    const user = userEvent.setup();
    await editFileAndMoveFlow();
    await user.click(within(screen.getByRole("banner")).getByRole("button", { name: "Save" }));
    const ask = await screen.findByRole("alertdialog", { name: `Save the open files before the flow moves to ${OTHER}?` });
    await user.click(within(ask).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(puts()).toHaveLength(0);
    expect(uploads()).toHaveLength(0);
    expect(within(strip(2)).getByRole("tab", { name: "a.py (/a.py), unsaved" })).toBeInTheDocument();
  });

  it("without unsaved files, saves at once and closes the old namespace's file tabs; no tab ever reads the new namespace", async () => {
    const user = userEvent.setup();
    storeLayout(layoutWith("file:a.py", "file:scripts/b.py"));
    renderFlow();
    await fileBox("b.py");
    type(await flowBox(), moved("moved"));
    await user.click(within(screen.getByRole("banner")).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    await waitFor(() => expect(tabNames(2)).toEqual(["Files"]));
    await waitFor(() => {
      const saved = JSON.parse(localStorage.getItem(LAYOUT_KEY) ?? "{}") as Layout;
      expect(saved.panels.flatMap((p) => p.tabs)).toEqual(["code", "files"]);
    }, LONG);
    expect(otherNamespaceFetches().filter((u) => !u.includes("/files/directory"))).toEqual([]);
    expect(toast.info).toHaveBeenCalledWith(`The flow is now in ${OTHER}, so the file tabs of ${NS} were closed.`);
  });

  it("a save that keeps the namespace asks nothing and keeps the file tabs", async () => {
    const user = userEvent.setup();
    storeLayout(layoutWith("file:a.py"));
    renderFlow();
    type(await fileBox("a.py"), "a = 2\n");
    type(await flowBox(), flowSource("same namespace"));
    await user.click(within(screen.getByRole("banner")).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(within(strip(2)).getByRole("tab", { name: "a.py (/a.py), unsaved" })).toBeInTheDocument();
  });
});
