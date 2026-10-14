import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import NamespaceFilesTab from "@/components/workflows/namespaces/NamespaceFilesTab";
import { FILES_WS, serveFiles, type FilesRequest, type FilesStore, type FilesWorld } from "./fakeFilesServer";

/**
 * A stand-in for the lazy Monaco wrapper: a text box named by the editor's
 * `ariaLabel`, carrying the language it was given, typed into like Monaco
 * (its `onChange` gets the whole text).
 */
vi.mock("@/components/monaco/MonacoEditor", () => ({
  default: (props: {
    language?: string;
    defaultValue?: string;
    onChange?: (v: string) => void;
    options?: { ariaLabel?: string; readOnly?: boolean };
  }) => (
    <textarea
      aria-label={props.options?.ariaLabel}
      data-language={props.language}
      readOnly={!!props.options?.readOnly}
      defaultValue={props.defaultValue}
      onChange={(e) => props.onChange?.(e.target.value)}
    />
  ),
}));
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), info: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const NS = "company.team";
const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00]);
const ZIP = new Uint8Array([0x50, 0x4b, 0x03, 0x04, 0x00, 0xff]);

/** The server lists in byte order; the tree shows folders first, then files, by name. */
const FILES: FilesWorld["files"] = {
  "/scripts/load.py": ["print('v1')\n", "print('v2')\n"],
  "/scripts/etl/clean.sql": "select 1;\n",
  "/archive/Scripts/old.py": "old = True\n",
  "/README.md": "# Team\n",
  "/assets/logo.png": PNG,
  "/bundle.zip": ZIP,
  "/a.py": "x = 1\n",
  "/data.bin": new Uint8Array([0x61, 0x00, 0xff]),
};

let requests: FilesRequest[] = [];
let store: FilesStore;

function renderTab({ canManage = true, world = {}, namespace = NS }: { canManage?: boolean; world?: Partial<FilesWorld>; namespace?: string } = {}) {
  ({ requests, store } = serveFiles({ namespace, files: FILES, ...world }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } } });
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter>
          <NamespaceFilesTab
            ws={FILES_WS}
            namespace={namespace}
            canManage={canManage}
            detail={{ namespace, flows: 0, files: 8, kv_keys: 0, own: true, can_manage: canManage }}
          />
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

const tree = () => screen.getByRole("tree", { name: `Files of ${NS}` });
const item = (name: string) => within(tree()).getByRole("treeitem", { name });
const findItem = async (name: string) => within(await screen.findByRole("tree", { name: `Files of ${NS}` })).findByRole("treeitem", { name });
const rowNames = (level: number) =>
  within(tree())
    .getAllByRole("treeitem")
    .filter((r) => r.getAttribute("aria-level") === String(level))
    .map((r) => r.getAttribute("aria-label"));
const sent = (method: string, route = "") => requests.filter((r) => r.method === method && r.route === route);
const editor = (name: string) => screen.findByRole("textbox", { name: `Contents of ${name}` });

async function openFile(user: ReturnType<typeof userEvent.setup>, ...names: string[]) {
  for (const n of names) {
    const row = await findItem(n);
    await user.click(row);
  }
}

beforeEach(() => {
  localStorage.clear();
  toast.success.mockClear();
  toast.error.mockClear();
  URL.createObjectURL = vi.fn(() => "blob:preview");
  URL.revokeObjectURL = vi.fn();
});
afterEach(() => vi.unstubAllGlobals());

describe("the file tree", () => {
  it("lists the root: folders first, then files, each by name, whatever order the server sends", async () => {
    renderTab();
    await findItem("a.py");
    expect(rowNames(1)).toEqual(["archive", "assets", "scripts", "a.py", "bundle.zip", "data.bin", "README.md"]);
    expect(item("scripts")).toHaveAttribute("aria-expanded", "false");
    expect(item("a.py")).not.toHaveAttribute("aria-expanded");
    expect(sent("GET", "/directory")[0].query.get("path")).toBeNull();
  });

  it("expands a folder through the directory route, one level deeper, and collapses it", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(await findItem("scripts"));
    expect(await findItem("load.py")).toHaveAttribute("aria-level", "2");
    expect(rowNames(2)).toEqual(["etl", "load.py"]);
    expect(item("scripts")).toHaveAttribute("aria-expanded", "true");
    expect(sent("GET", "/directory").map((r) => r.query.get("path"))).toContain("/scripts/");
    await user.click(item("scripts"));
    expect(within(tree()).queryByRole("treeitem", { name: "load.py" })).toBeNull();
  });

  it("works from the keyboard: arrows move and expand, Left goes back to the parent, Enter opens", async () => {
    const user = userEvent.setup();
    renderTab();
    (await findItem("assets")).focus();
    await user.keyboard("{ArrowDown}");
    expect(item("scripts")).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    await findItem("etl");
    await user.keyboard("{ArrowRight}");
    expect(item("etl")).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(item("load.py")).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(item("scripts")).toHaveFocus();
    await user.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    expect(await editor("load.py")).toBeInTheDocument();
  });

  it("searches through W9b's search route and opens a result", async () => {
    const user = userEvent.setup();
    renderTab();
    await findItem("a.py");
    await user.type(screen.getByLabelText("Search files"), "clean");
    const results = await screen.findByRole("list", { name: "Search results" });
    expect(sent("GET", "/search").at(-1)!.query.get("q")).toBe("clean");
    await user.click(within(results).getByRole("button", { name: /clean\.sql/ }));
    expect(await editor("clean.sql")).toHaveValue("select 1;\n");
  });

  it("says when a search finds nothing, and clears it", async () => {
    const user = userEvent.setup();
    renderTab();
    await findItem("a.py");
    await user.type(screen.getByLabelText("Search files"), "zzz");
    expect(await screen.findByText("No files match “zzz”.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Clear search" }));
    expect(await findItem("a.py")).toBeInTheDocument();
  });

  it("shows an empty namespace as empty, with its first actions", async () => {
    renderTab({ world: { files: {} } });
    expect(await screen.findByText("No files yet")).toBeInTheDocument();
  });
});

describe("creating and importing", () => {
  it("creates a file in the selected folder with if_revision=0, then opens it", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(await findItem("scripts"));
    await user.click(screen.getByRole("button", { name: "New file" }));
    const dialog = await screen.findByRole("dialog", { name: "New file" });
    expect(within(dialog).getByText("Created in /scripts/.")).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("Name"), "jobs/run.py");
    await user.click(within(dialog).getByRole("button", { name: "Create file" }));
    expect(await editor("run.py")).toHaveValue("");
    const post = sent("POST").at(-1)!;
    expect(post.query.get("path")).toBe("/scripts/jobs/run.py");
    expect(post.query.get("if_revision")).toBe("0");
    expect(post.upload).toBe("");
    expect(await findItem("run.py")).toHaveAttribute("aria-level", "3");
  });

  it("does not replace an existing file: the server's 409 shows beside the name, which is kept", async () => {
    const user = userEvent.setup();
    renderTab();
    await findItem("a.py");
    await user.click(screen.getByRole("button", { name: "New file" }));
    const dialog = await screen.findByRole("dialog", { name: "New file" });
    await user.type(within(dialog).getByLabelText("Name"), "a.py");
    await user.click(within(dialog).getByRole("button", { name: "Create file" }));
    expect(await within(dialog).findByTestId("file-dialog-error")).toHaveTextContent(
      "Namespace file '/a.py' already exists in namespace 'company.team' (revision 1); nothing was written. Choose another name.",
    );
    expect(within(dialog).getByLabelText("Name")).toHaveValue("a.py");
    expect(store.text("/a.py")).toBe("x = 1\n");
  });

  it("creates a folder at the root", async () => {
    const user = userEvent.setup();
    renderTab();
    await findItem("a.py");
    await user.click(screen.getByRole("button", { name: "New folder" }));
    const dialog = await screen.findByRole("dialog", { name: "New folder" });
    await user.type(within(dialog).getByLabelText("Name"), "lib");
    await user.click(within(dialog).getByRole("button", { name: "Create folder" }));
    expect(await findItem("lib")).toHaveAttribute("aria-level", "1");
    expect(sent("POST", "/directory")[0].query.get("path")).toBe("/lib/");
  });

  it("imports files (a .zip as one file); one that exists is asked about, then overwritten", async () => {
    const user = userEvent.setup();
    renderTab();
    await findItem("a.py");
    const files = [new File(["y = 2\n"], "a.py"), new File(["new"], "new.txt"), new File([ZIP], "pack.zip")];
    await user.upload(screen.getByLabelText("Choose files to import"), files);
    const ask = await screen.findByRole("alertdialog", { name: "Overwrite 1 file?" });
    expect(within(ask).getByRole("list", { name: "Files that already exist" })).toHaveTextContent("/a.py");
    // Every file went with if_revision=0; nothing existing was replaced yet.
    expect(sent("POST").map((r) => [r.query.get("path"), r.query.get("if_revision")])).toEqual([
      ["/a.py", "0"],
      ["/new.txt", "0"],
      ["/pack.zip", "0"],
    ]);
    expect(store.files.get("/pack.zip")).toHaveLength(1);
    expect(store.text("/a.py")).toBe("x = 1\n");
    await user.click(within(ask).getByRole("button", { name: "Overwrite" }));
    await waitFor(() => expect(store.text("/a.py")).toBe("y = 2\n"));
    const last = sent("POST").at(-1)!;
    expect(last.query.get("path")).toBe("/a.py");
    expect(last.query.has("if_revision")).toBe(false);
    expect(await findItem("new.txt")).toBeInTheDocument();
  });

  it("keeps the existing files when asked to", async () => {
    const user = userEvent.setup();
    renderTab();
    await findItem("a.py");
    await user.upload(screen.getByLabelText("Choose files to import"), [new File(["y"], "a.py")]);
    const ask = await screen.findByRole("alertdialog", { name: "Overwrite 1 file?" });
    await user.click(within(ask).getByRole("button", { name: "Keep the existing files" }));
    expect(sent("POST")).toHaveLength(1);
    expect(store.text("/a.py")).toBe("x = 1\n");
  });

  it("imports a folder (webkitdirectory), each file at its path below it, into the selected folder", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(await findItem("scripts"));
    const input = screen.getByLabelText("Choose a folder to import");
    expect(input).toHaveAttribute("webkitdirectory");
    const m = new File(["m = 1\n"], "m.py");
    Object.defineProperty(m, "webkitRelativePath", { value: "proj/src/m.py" });
    const z = new File([ZIP], "data.zip");
    Object.defineProperty(z, "webkitRelativePath", { value: "proj/data.zip" });
    await user.upload(input, [m, z]);
    await waitFor(() => expect(sent("POST")).toHaveLength(2));
    expect(sent("POST").map((r) => r.query.get("path"))).toEqual(["/scripts/proj/src/m.py", "/scripts/proj/data.zip"]);
    expect(store.text("/scripts/proj/src/m.py")).toBe("m = 1\n");
    expect(await findItem("proj")).toHaveAttribute("aria-level", "2");
    expect(toast.success).toHaveBeenCalledWith("Imported 2 files into /scripts/.");
  });

  it("keeps every write off while an import runs, saying why", async () => {
    const user = userEvent.setup();
    let open!: () => void;
    renderTab({ world: { uploadGate: new Promise<void>((r) => (open = r)) } });
    await findItem("a.py");
    await user.upload(screen.getByLabelText("Choose files to import"), [new File(["1"], "one.txt"), new File(["2"], "two.txt")]);
    expect(await screen.findByRole("status")).toHaveTextContent("Importing 0 of 2 files");
    for (const name of ["New file", "New folder", "Import files", "Import a folder"]) {
      const b = screen.getByRole("button", { name });
      expect(b).toBeDisabled();
      expect(b).toHaveAccessibleDescription("Wait for the import to finish.");
    }
    // No drag and drop, and no F2 rename, while it runs.
    expect(item("a.py")).toHaveAttribute("draggable", "false");
    await user.click(item("a.py"));
    await user.keyboard("{F2}");
    expect(screen.queryByRole("dialog", { name: "Rename a.py" })).not.toBeInTheDocument();

    open();
    await waitFor(() => expect(sent("POST")).toHaveLength(2));
    await waitFor(() => expect(screen.getByRole("button", { name: "Import files" })).toBeEnabled());
    expect(item("a.py")).toHaveAttribute("draggable", "true");
  });

  it("asks before importing more than 100 files: Cancel sends nothing, Import sends them all", async () => {
    const user = userEvent.setup();
    renderTab();
    await findItem("a.py");
    const many = Array.from({ length: 101 }, (_, i) => new File([String(i)], `f${String(i).padStart(3, "0")}.txt`));
    await user.upload(screen.getByLabelText("Choose files to import"), many);
    let ask = await screen.findByRole("alertdialog", { name: "Import 101 files?" });
    await user.click(within(ask).getByRole("button", { name: "Cancel" }));
    expect(sent("POST")).toHaveLength(0);

    await user.upload(screen.getByLabelText("Choose files to import"), many);
    ask = await screen.findByRole("alertdialog", { name: "Import 101 files?" });
    await user.click(within(ask).getByRole("button", { name: "Import 101 files" }));
    await waitFor(() => expect(sent("POST")).toHaveLength(101), { timeout: 10_000 });

    // A hundred is not asked about.
    await waitFor(() => expect(screen.getByRole("button", { name: "Import files" })).toBeEnabled());
    await user.upload(screen.getByLabelText("Choose files to import"), many.slice(0, 100).map((f) => new File(["x"], `again-${f.name}`)));
    await waitFor(() => expect(sent("POST")).toHaveLength(201), { timeout: 10_000 });
    expect(screen.queryByRole("alertdialog", { name: /^Import/ })).not.toBeInTheDocument();
  });

  it("shows the server's 413 for a file over the cap, and imports the rest", async () => {
    const user = userEvent.setup();
    renderTab({
      world: { uploadReplies: { "/big.bin": { status: 413, body: { error: "the file is larger than workflow.storage.max_file_bytes (10485760 bytes)" } } } },
    });
    await findItem("a.py");
    await user.upload(screen.getByLabelText("Choose files to import"), [new File(["x"], "big.bin"), new File(["ok"], "ok.txt")]);
    const alert = await screen.findByText("1 file was not imported:");
    expect(alert.closest("[role=alert]")).toHaveTextContent(
      "/big.bin was not imported: the file is larger than workflow.storage.max_file_bytes (10485760 bytes). Make it smaller and try again.",
    );
    expect(store.text("/ok.txt")).toBe("ok");
  });
});

describe("export, rename and delete", () => {
  it("exports through W9b's export route", async () => {
    renderTab();
    await findItem("a.py");
    expect(screen.getByRole("link", { name: "Export as zip" })).toHaveAttribute(
      "href",
      `/api/ws/${FILES_WS}/workflows/namespaces/${NS}/files/export`,
    );
  });

  it("renames a file from its menu: a move in the same folder", async () => {
    const user = userEvent.setup();
    renderTab();
    await findItem("a.py");
    await user.click(within(item("a.py")).getByRole("button", { name: "Actions for a.py" }));
    await user.click(await screen.findByRole("menuitem", { name: "Rename" }));
    const dialog = await screen.findByRole("dialog", { name: "Rename a.py" });
    const field = within(dialog).getByLabelText("New name");
    expect(field).toHaveValue("a.py");
    await user.clear(field);
    await user.type(field, "b.py");
    await user.click(within(dialog).getByRole("button", { name: "Rename" }));
    expect(await findItem("b.py")).toBeInTheDocument();
    const put = sent("PUT")[0];
    expect([put.query.get("from"), put.query.get("to")]).toEqual(["/a.py", "/b.py"]);
  });

  it("renames with F2 and refuses a slash in the name", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(await findItem("a.py"));
    item("a.py").focus();
    await user.keyboard("{F2}");
    const dialog = await screen.findByRole("dialog", { name: "Rename a.py" });
    await user.clear(within(dialog).getByLabelText("New name"));
    await user.type(within(dialog).getByLabelText("New name"), "x/b.py");
    await user.click(within(dialog).getByRole("button", { name: "Rename" }));
    expect(within(dialog).getByTestId("file-dialog-error")).toHaveTextContent("A name cannot contain /");
    expect(sent("PUT")).toHaveLength(0);
  });

  it("deletes a folder after a confirmation naming how many files go (only those under it)", async () => {
    const user = userEvent.setup();
    renderTab();
    await findItem("scripts");
    await user.click(within(item("scripts")).getByRole("button", { name: "Actions for scripts" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const ask = await screen.findByRole("alertdialog", { name: "Delete the folder scripts?" });
    // `/archive/Scripts/old.py` matches the search too (case-insensitive substring), and is not counted.
    expect(await within(ask).findByText(/This deletes the folder scripts and the 2 files in it\./)).toBeInTheDocument();
    await user.click(within(ask).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(within(tree()).queryByRole("treeitem", { name: "scripts" })).toBeNull());
    expect(sent("DELETE")[0].query.get("path")).toBe("/scripts/");
    expect(store.live("/scripts/load.py")).toBeUndefined();
    expect(store.live("/archive/Scripts/old.py")).toBeDefined();
  });

  it("deletes a file after naming it, with the Delete key", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(await findItem("a.py"));
    item("a.py").focus();
    await user.keyboard("{Delete}");
    const ask = await screen.findByRole("alertdialog", { name: "Delete a.py?" });
    expect(within(ask).getByText(/This deletes the file a\.py\./)).toBeInTheDocument();
    await user.click(within(ask).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(sent("DELETE")).toHaveLength(1));
    // The open tab of the deleted file is closed.
    await waitFor(() => expect(screen.queryByRole("tab", { name: "a.py" })).toBeNull());
  });
});

describe("moving", () => {
  const dataTransfer = () => ({ setData: vi.fn(), getData: vi.fn(), dropEffect: "", effectAllowed: "" });

  it("moves a file by drag and drop onto a folder", async () => {
    renderTab();
    await findItem("a.py");
    const dt = dataTransfer();
    fireEvent.dragStart(item("a.py"), { dataTransfer: dt });
    fireEvent.dragOver(item("scripts"), { dataTransfer: dt });
    fireEvent.drop(item("scripts"), { dataTransfer: dt });
    await waitFor(() => expect(sent("PUT")).toHaveLength(1));
    expect([sent("PUT")[0].query.get("from"), sent("PUT")[0].query.get("to")]).toEqual(["/a.py", "/scripts/a.py"]);
    expect(await findItem("a.py")).toHaveAttribute("aria-level", "2");
  });

  it("does not drop a folder into itself", async () => {
    renderTab();
    await findItem("scripts");
    const dt = dataTransfer();
    fireEvent.dragStart(item("scripts"), { dataTransfer: dt });
    fireEvent.drop(item("scripts"), { dataTransfer: dt });
    await new Promise((r) => setTimeout(r, 20));
    expect(sent("PUT")).toHaveLength(0);
  });

  it("moves from the keyboard: Shift+F10 opens the menu, Move to… picks the folder", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(await findItem("a.py"));
    item("a.py").focus();
    await user.keyboard("{Shift>}{F10}{/Shift}");
    await user.click(await screen.findByRole("menuitem", { name: "Move to…" }));
    const dialog = await screen.findByRole("dialog", { name: "Move a.py" });
    const field = within(dialog).getByLabelText("Destination folder");
    expect(field).toHaveValue("/");
    await user.clear(field);
    await user.type(field, "scripts/etl");
    await user.click(within(dialog).getByRole("button", { name: "Move" }));
    await waitFor(() => expect(sent("PUT")).toHaveLength(1));
    expect(sent("PUT")[0].query.get("to")).toBe("/scripts/etl/a.py");
    expect(store.text("/scripts/etl/a.py")).toBe("x = 1\n");
  });

  it("refuses to move a folder into a folder inside it", async () => {
    const user = userEvent.setup();
    renderTab();
    await findItem("scripts");
    await user.click(within(item("scripts")).getByRole("button", { name: "Actions for scripts" }));
    await user.click(await screen.findByRole("menuitem", { name: "Move to…" }));
    const dialog = await screen.findByRole("dialog", { name: "Move scripts" });
    await user.clear(within(dialog).getByLabelText("Destination folder"));
    await user.type(within(dialog).getByLabelText("Destination folder"), "/scripts/etl/");
    await user.click(within(dialog).getByRole("button", { name: "Move" }));
    expect(within(dialog).getByTestId("file-dialog-error")).toHaveTextContent("A folder cannot move into itself");
    expect(sent("PUT")).toHaveLength(0);
  });

  it("refuses to move a file with unsaved edits", async () => {
    const user = userEvent.setup();
    renderTab();
    await openFile(user, "a.py");
    await user.type(await editor("a.py"), "!");
    fireEvent.dragStart(item("a.py"), { dataTransfer: dataTransfer() });
    fireEvent.drop(item("scripts"), { dataTransfer: dataTransfer() });
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("a.py has unsaved edits. Save or close it first."));
    expect(sent("PUT")).toHaveLength(0);
  });
});

describe("revisions", () => {
  it("lists them newest first, views one, and restores one as a new revision", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(await findItem("scripts"));
    await openFile(user, "load.py");
    expect(await editor("load.py")).toHaveValue("print('v2')\n");
    await user.click(screen.getByRole("button", { name: "Revisions" }));
    const drawer = await screen.findByRole("dialog", { name: "Revisions of load.py" });
    const list = await within(drawer).findByRole("list", { name: "Revisions of load.py" });
    expect(within(list).getAllByRole("listitem").map((li) => li.querySelector("p")?.textContent)).toEqual(["Revision 2Current", "Revision 1"]);
    expect(within(drawer).getByRole("button", { name: "Restore revision 2" })).toBeDisabled();

    await user.click(within(drawer).getByRole("button", { name: "View revision 1" }));
    expect(await within(drawer).findByRole("textbox", { name: "Revision 1 of load.py" })).toHaveValue("print('v1')\n");

    await user.click(within(drawer).getByRole("button", { name: "Restore revision 1" }));
    await waitFor(() => expect(store.text("/scripts/load.py")).toBe("print('v1')\n"));
    const post = sent("POST").at(-1)!;
    expect([post.query.get("path"), post.query.get("if_revision"), post.upload]).toEqual(["/scripts/load.py", "2", "print('v1')\n"]);
    expect(store.files.get("/scripts/load.py")).toHaveLength(3);
    expect(toast.success).toHaveBeenCalledWith("Restored revision 1 of load.py as a new revision.");
    // The open file had no edits, so it reads the restored revision.
    await user.keyboard("{Escape}");
    await waitFor(async () => expect(await editor("load.py")).toHaveValue("print('v1')\n"));
    expect(screen.getByText(/Revision 3/)).toBeInTheDocument();
  });

  it("asks before restoring over a revision written since the list was read: List again, or restore anyway", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(await findItem("scripts"));
    await user.click(await findItem("load.py"));
    await user.click(within(item("load.py")).getByRole("button", { name: "Actions for load.py" }));
    await user.click(await screen.findByRole("menuitem", { name: "Revisions" }));
    const drawer = await screen.findByRole("dialog", { name: "Revisions of load.py" });
    await within(drawer).findByRole("list", { name: "Revisions of load.py" });
    store.write("/scripts/load.py", new TextEncoder().encode("print('v3')\n"));

    await user.click(within(drawer).getByRole("button", { name: "Restore revision 1" }));
    let ask = await screen.findByRole("alertdialog", { name: "load.py changed since its revisions were listed" });
    expect(ask).toHaveTextContent("was changed: it is at revision 3, not 2; nothing was written.");
    expect(store.text("/scripts/load.py")).toBe("print('v3')\n");
    await user.click(within(ask).getByRole("button", { name: "List again" }));
    expect(await within(drawer).findByRole("button", { name: "View revision 3" })).toBeInTheDocument();
    expect(store.files.get("/scripts/load.py")).toHaveLength(3);

    store.write("/scripts/load.py", new TextEncoder().encode("print('v4')\n"));
    await user.click(within(drawer).getByRole("button", { name: "Restore revision 1" }));
    ask = await screen.findByRole("alertdialog", { name: "load.py changed since its revisions were listed" });
    await user.click(within(ask).getByRole("button", { name: "Restore anyway" }));
    await waitFor(() => expect(store.text("/scripts/load.py")).toBe("print('v1')\n"));
    expect(sent("POST").at(-1)!.query.has("if_revision")).toBe(false);
  });
});

describe("revisions over 5 MB", () => {
  it("offers a revision over 5 MB for download only, without reading it; a small one is still shown", async () => {
    const user = userEvent.setup();
    const big = new Uint8Array(5 * 1024 * 1024 + 1).fill(0x61);
    renderTab({ world: { files: { ...FILES, "/big.txt": ["small\n", big] } } });
    await user.click(await findItem("big.txt"));
    await user.click(within(item("big.txt")).getByRole("button", { name: "Actions for big.txt" }));
    await user.click(await screen.findByRole("menuitem", { name: "Revisions" }));
    const drawer = await screen.findByRole("dialog", { name: "Revisions of big.txt" });
    await within(drawer).findByRole("list", { name: "Revisions of big.txt" });

    await user.click(within(drawer).getByRole("button", { name: "View revision 2" }));
    expect(await within(drawer).findByText(/Revision 2 is larger than 5 MB, so it is not shown here/)).toBeInTheDocument();
    expect(within(drawer).getByRole("link", { name: "Download revision 2" }).getAttribute("href")).toBe(
      `/api/ws/${FILES_WS}/workflows/namespaces/${NS}/files?path=%2Fbig.txt&revision=2`,
    );
    expect(within(drawer).queryByRole("textbox", { name: "Revision 2 of big.txt" })).toBeNull();
    expect(sent("GET").filter((r) => r.query.get("path") === "/big.txt" && r.query.get("revision") === "2")).toHaveLength(0);

    await user.click(within(drawer).getByRole("button", { name: "View revision 1" }));
    expect(await within(drawer).findByRole("textbox", { name: "Revision 1 of big.txt" })).toHaveValue("small\n");
  });
});

describe("opening a file", () => {
  it("reads the revision from stats, then the bytes AT that revision, into Monaco with the extension's language", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(await findItem("scripts"));
    await openFile(user, "load.py");
    const box = await editor("load.py");
    expect(box).toHaveValue("print('v2')\n");
    expect(box).toHaveAttribute("data-language", "python");
    expect(sent("GET", "/stats")[0].query.get("path")).toBe("/scripts/load.py");
    const read = sent("GET").find((r) => r.query.get("path") === "/scripts/load.py")!;
    expect(read.query.get("revision")).toBe("2");
    expect(screen.getByText(/Revision 2/)).toBeInTheDocument();
  });

  it("previews an image", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(await findItem("assets"));
    await openFile(user, "logo.png");
    expect(await screen.findByRole("img", { name: "Preview of logo.png" })).toHaveAttribute("src", "blob:preview");
  });

  it("offers a known binary for download only, without reading it", async () => {
    const user = userEvent.setup();
    renderTab();
    await openFile(user, "bundle.zip");
    const link = await screen.findByRole("link", { name: "Download bundle.zip" });
    expect(link.getAttribute("href")).toBe(`/api/ws/${FILES_WS}/workflows/namespaces/${NS}/files?path=%2Fbundle.zip&revision=1`);
    expect(screen.queryByRole("textbox", { name: "Contents of bundle.zip" })).toBeNull();
    expect(sent("GET").filter((r) => r.query.get("path") === "/bundle.zip")).toHaveLength(0);
  });

  it("calls a file binary by its bytes when its extension says nothing", async () => {
    const user = userEvent.setup();
    renderTab();
    await openFile(user, "data.bin");
    expect(await screen.findByText(/data\.bin is a binary file, so it cannot be shown here/)).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Contents of data.bin" })).toBeNull();
  });
});

describe("saving", () => {
  it("sends if_revision; the file is then at the next revision", async () => {
    const user = userEvent.setup();
    renderTab();
    await openFile(user, "a.py");
    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    expect(save).toHaveAccessibleDescription("No unsaved changes");
    const box = await editor("a.py");
    await user.clear(box);
    await user.type(box, "x = 2");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(store.text("/a.py")).toBe("x = 2"));
    const post = sent("POST").at(-1)!;
    expect([post.query.get("path"), post.query.get("if_revision"), post.upload]).toEqual(["/a.py", "1", "x = 2"]);
    expect(await screen.findByText(/Revision 2/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    // The next save sends the revision the first one produced.
    await user.type(box, "!");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(sent("POST").at(-1)!.query.get("if_revision")).toBe("2"));
  });

  it("saves with Ctrl+S in the editor", async () => {
    const user = userEvent.setup();
    renderTab();
    await openFile(user, "a.py");
    const box = await editor("a.py");
    await user.type(box, "y");
    await user.keyboard("{Control>}s{/Control}");
    await waitFor(() => expect(store.text("/a.py")).toBe("x = 1\ny"));
  });

  it("a 409 file_changed opens the stale-save dialog; Overwrite resends without if_revision", async () => {
    const user = userEvent.setup();
    renderTab();
    await openFile(user, "a.py");
    const box = await editor("a.py");
    await user.type(box, "mine");
    store.write("/a.py", new TextEncoder().encode("theirs\n"));
    await user.click(screen.getByRole("button", { name: "Save" }));
    const ask = await screen.findByRole("alertdialog", { name: "a.py changed since you opened it" });
    expect(ask).toHaveTextContent("Namespace file '/a.py' in namespace 'company.team' was changed: it is at revision 2, not 1; nothing was written.");
    expect(store.text("/a.py")).toBe("theirs\n");
    await user.click(within(ask).getByRole("button", { name: "Overwrite" }));
    await waitFor(() => expect(store.text("/a.py")).toBe("x = 1\nmine"));
    expect(sent("POST").at(-1)!.query.has("if_revision")).toBe(false);
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(await screen.findByText(/Revision 3/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("Reload, after its own question, reads the latest revision and drops the edits", async () => {
    const user = userEvent.setup();
    renderTab();
    await openFile(user, "a.py");
    await user.type(await editor("a.py"), "mine");
    store.write("/a.py", new TextEncoder().encode("theirs\n"));
    await user.click(screen.getByRole("button", { name: "Save" }));
    const ask = await screen.findByRole("alertdialog", { name: "a.py changed since you opened it" });
    await user.click(within(ask).getByRole("button", { name: "Reload" }));
    await user.click(within(ask).getByRole("button", { name: "Discard and reload" }));
    await waitFor(async () => expect(await editor("a.py")).toHaveValue("theirs\n"));
    expect(screen.getByText(/Revision 2/)).toBeInTheDocument();
    expect(store.files.get("/a.py")).toHaveLength(2);
  });

  it("a file deleted meanwhile is the same dialog, saying so; Overwrite writes it again", async () => {
    const user = userEvent.setup();
    renderTab();
    await openFile(user, "a.py");
    await user.type(await editor("a.py"), "!");
    store.files.get("/a.py")!.at(-1)!.deleted = true;
    await user.click(screen.getByRole("button", { name: "Save" }));
    const ask = await screen.findByRole("alertdialog", { name: "a.py changed since you opened it" });
    expect(ask).toHaveTextContent("was deleted (at revision 1)");
    await user.click(within(ask).getByRole("button", { name: "Overwrite" }));
    await waitFor(() => expect(store.text("/a.py")).toBe("x = 1\n!"));
  });

  it("a 409 without file_changed is an error with the server's message, not the dialog; the text is kept", async () => {
    const user = userEvent.setup();
    renderTab({ world: { uploadReplies: { "/a.py": { status: 409, body: { error: "conflict: the path is busy (a move or another write holds it); try again" } } } } });
    await openFile(user, "a.py");
    const box = await editor("a.py");
    await user.type(box, "!");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("The file was not saved: the path is busy (a move or another write holds it); try again.")).toBeInTheDocument();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(box).toHaveValue("x = 1\n!");
  });

  it("a 403 is shown as a refusal that names the permission", async () => {
    const user = userEvent.setup();
    renderTab({ world: { uploadReplies: { "/a.py": { status: 403, body: { error: "missing permission workflow:manage" } } } } });
    await openFile(user, "a.py");
    await user.type(await editor("a.py"), "!");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(
      await screen.findByText(
        "The file was not saved: missing permission workflow:manage. Changing namespace files needs the workflow:manage permission; ask a workspace admin for it.",
      ),
    ).toBeInTheDocument();
  });
});

describe("open tabs", () => {
  it("shows a dot on a tab with unsaved edits, and asks before closing it", async () => {
    const user = userEvent.setup();
    renderTab();
    await openFile(user, "a.py", "README.md");
    const tabs = screen.getByRole("tablist", { name: "Open files" });
    expect(within(tabs).getAllByRole("tab").map((t) => t.textContent)).toEqual(["a.py", "README.md"]);
    expect(within(tabs).getByRole("tab", { name: "README.md" })).toHaveAttribute("aria-selected", "true");
    await user.click(within(tabs).getByRole("tab", { name: "a.py" }));
    await user.type(await editor("a.py"), "!");
    expect(within(tabs).getByRole("tab", { name: "a.py, unsaved" })).toBeInTheDocument();
    expect(within(tabs).getByTestId("unsaved-dot")).toBeInTheDocument();

    await user.click(within(tabs).getByRole("button", { name: "Close a.py" }));
    const ask = await screen.findByRole("alertdialog", { name: "Close a.py without saving?" });
    await user.click(within(ask).getByRole("button", { name: "Keep editing" }));
    expect(within(tabs).getByRole("tab", { name: "a.py, unsaved" })).toBeInTheDocument();
    expect(await editor("a.py")).toHaveValue("x = 1\n!");

    await user.click(within(tabs).getByRole("button", { name: "Close a.py" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Discard and close" }));
    await waitFor(() => expect(within(tabs).queryByRole("tab", { name: /a\.py/ })).toBeNull());
    // A clean tab closes without asking.
    await user.click(within(tabs).getByRole("button", { name: "Close README.md" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByText("No file open")).toBeInTheDocument();
  });

  it("keeps the text of a tab in the background", async () => {
    const user = userEvent.setup();
    renderTab();
    await openFile(user, "a.py");
    await user.type(await editor("a.py"), "!");
    await openFile(user, "README.md");
    await user.click(screen.getByRole("tab", { name: "a.py, unsaved" }));
    expect(await editor("a.py")).toHaveValue("x = 1\n!");
  });

  it("remembers the open tabs per namespace", async () => {
    const user = userEvent.setup();
    const { unmount } = renderTab();
    await openFile(user, "a.py", "README.md");
    await user.click(screen.getByRole("tab", { name: "a.py" }));
    unmount();
    renderTab();
    const tabs = await screen.findByRole("tablist", { name: "Open files" });
    expect(within(tabs).getAllByRole("tab").map((t) => t.textContent)).toEqual(["a.py", "README.md"]);
    expect(within(tabs).getByRole("tab", { name: "a.py" })).toHaveAttribute("aria-selected", "true");
    expect(await editor("a.py")).toHaveValue("x = 1\n");
    cleanup();
    renderTab({ namespace: "company.other", world: { files: { "/z.py": "z\n" } } });
    expect(await screen.findByText("No file open")).toBeInTheDocument();
  });
});

describe("errors", () => {
  it("shows the server's refusal of a namespace that cannot hold files", async () => {
    renderTab({ world: { replies: { "/directory": { status: 400, body: { error: "invalid namespace 'company..team': a segment is empty" } } } } });
    expect(
      await screen.findByText(/The namespace company\.team cannot hold files: Invalid namespace 'company\.\.team': a segment is empty\./),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });

  it("shows the server's refusal of a _files path beside the name", async () => {
    const user = userEvent.setup();
    renderTab({
      world: { uploadReplies: { "/x/_files/y.py": { status: 400, body: { error: "the path '/x/_files/y.py' has a `_files` segment, which namespace file paths cannot hold" } } } },
    });
    await findItem("a.py");
    await user.click(screen.getByRole("button", { name: "New file" }));
    const dialog = await screen.findByRole("dialog", { name: "New file" });
    await user.type(within(dialog).getByLabelText("Name"), "x/_files/y.py");
    await user.click(within(dialog).getByRole("button", { name: "Create file" }));
    expect(await within(dialog).findByTestId("file-dialog-error")).toHaveTextContent(
      "The file was not created: the path '/x/_files/y.py' has a `_files` segment, which namespace file paths cannot hold. Correct it and try again.",
    );
  });

  it("says when the server cannot be reached, with Try again", async () => {
    renderTab();
    vi.stubGlobal("fetch", vi.fn(async () => Promise.reject(new TypeError("Failed to fetch"))));
    cleanup();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <MemoryRouter>
          <NamespaceFilesTab ws={FILES_WS} namespace={NS} canManage detail={{ namespace: NS, flows: 0, files: 0, kv_keys: 0, own: true, can_manage: true }} />
        </MemoryRouter>
      </QueryClientProvider>,
    );
    expect(
      await screen.findByText("The files could not be listed: the server could not be reached. Check your connection and try again."),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
  });
});

describe("a read-only viewer (no workflow:manage)", () => {
  const REASON = "Changing namespace files needs the workflow:manage permission";

  it("sees every file and the export, and every write is disabled with its reason", async () => {
    const user = userEvent.setup();
    renderTab({ canManage: false });
    await findItem("a.py");
    for (const name of ["New file", "New folder", "Import files", "Import a folder"]) {
      const b = screen.getByRole("button", { name });
      expect(b).toBeDisabled();
      expect(b).toHaveAccessibleDescription(REASON);
    }
    expect(screen.getByRole("link", { name: "Export as zip" })).toBeInTheDocument();
    expect(item("a.py")).toHaveAttribute("draggable", "false");

    await user.click(within(item("a.py")).getByRole("button", { name: "Actions for a.py" }));
    for (const name of ["Rename", "Move to…", "Delete"]) {
      const entry = await screen.findByRole("menuitem", { name: new RegExp(`^${name}`) });
      expect(entry).toHaveAttribute("aria-disabled", "true");
      expect(entry).toHaveTextContent(REASON);
    }
    expect(screen.getByRole("menuitem", { name: "Revisions" })).not.toHaveAttribute("aria-disabled");
    await user.keyboard("{Escape}");

    await openFile(user, "a.py");
    const box = await editor("a.py");
    expect(box).toHaveAttribute("readonly");
    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    expect(save).toHaveAccessibleDescription(REASON);
  });

  it("is not told about F2 and Delete, which do nothing for it", async () => {
    renderTab({ canManage: false });
    await findItem("a.py");
    expect(tree()).toHaveAccessibleDescription("Arrow keys move through the files. Enter opens a file, and Shift+F10 opens a file's actions.");
  });

  it("cannot restore a revision, and is told why", async () => {
    const user = userEvent.setup();
    renderTab({ canManage: false });
    await user.click(await findItem("scripts"));
    await openFile(user, "load.py");
    await user.click(await screen.findByRole("button", { name: "Revisions" }));
    const drawer = await screen.findByRole("dialog", { name: "Revisions of load.py" });
    const restore = await within(drawer).findByRole("button", { name: "Restore revision 1" });
    expect(restore).toBeDisabled();
    expect(restore).toHaveAccessibleDescription(REASON);
  });
});
