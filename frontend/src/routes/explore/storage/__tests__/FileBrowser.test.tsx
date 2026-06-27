import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// ── Mock factories ────────────────────────────────────────────────────────────
const objectsMock = vi.fn();
const refetch = vi.fn();
const uploadMutation = vi.fn();
const uploadMutate = vi.fn();
const deleteMutation = vi.fn();
const deleteMutate = vi.fn();
const createFolderMutation = vi.fn();
const createFolderMutate = vi.fn();
const moveMutation = vi.fn();
const moveMutate = vi.fn();
const downloadObject = vi.fn();

vi.mock("@/api/storage", () => ({
  useStorageObjects: (_ws: string, _id: string, prefix: string) =>
    objectsMock(prefix),
  useUploadObject: () => uploadMutation(),
  useDeleteObject: () => deleteMutation(),
  useCreateFolder: () => createFolderMutation(),
  useMoveObject: () => moveMutation(),
  downloadObject: (...args: unknown[]) => downloadObject(...args),
}));

import FileBrowser from "@/routes/explore/storage/FileBrowser";
import type { StorageConn } from "@/api/storage";

const conn: StorageConn = {
  id: "conn-1",
  name: "prod-lakehouse",
  purpose: "catalog",
  provider: "s3",
  endpoint: "https://minio.corp.local",
  region: "us-east-1",
  bucket: "my-data-lake",
  key_prefix: null,
  access_key: "AKIA",
  path_style: true,
  created_at: "2025-01-01T00:00:00Z",
  has_secret: true,
};

const listing = {
  prefix: "",
  folders: ["sub/"],
  objects: [
    { key: "readme.txt", size: 1234, last_modified: "2025-01-01T00:00:00Z" },
    { key: "data.csv", size: 5678, last_modified: "2025-01-02T00:00:00Z" },
    { key: "events.parquet", size: 9012, last_modified: "2025-01-03T00:00:00Z" },
  ],
};

function ok(data: typeof listing) {
  return { data, isLoading: false, isError: false, error: null, refetch };
}

/** Open the ⋮ menu for the row identified by its action button aria-label. */
async function openMenu(name: RegExp) {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name }));
  return user;
}

beforeEach(() => {
  objectsMock.mockReset();
  refetch.mockReset();
  uploadMutation.mockReset();
  uploadMutate.mockReset();
  deleteMutation.mockReset();
  deleteMutate.mockReset();
  createFolderMutation.mockReset();
  createFolderMutate.mockReset();
  moveMutation.mockReset();
  moveMutate.mockReset();
  downloadObject.mockReset();

  objectsMock.mockReturnValue(ok(listing));
  uploadMutation.mockReturnValue({ mutate: uploadMutate, isPending: false });
  deleteMutation.mockReturnValue({ mutate: deleteMutate, isPending: false });
  createFolderMutation.mockReturnValue({
    mutate: createFolderMutate,
    isPending: false,
  });
  moveMutation.mockReturnValue({ mutate: moveMutate, isPending: false });
  downloadObject.mockResolvedValue(undefined);
});

describe("FileBrowser", () => {
  it("renders the connection info header with name and meta", () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    expect(screen.getByText("prod-lakehouse")).toBeInTheDocument();
    // meta line: purpose · provider · bucket
    expect(
      screen.getByText("catalog · S3 / MinIO · my-data-lake"),
    ).toBeInTheDocument();
  });

  it("renders folders and files from the listing", () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    expect(screen.getByText("sub")).toBeInTheDocument();
    expect(screen.getByText("readme.txt")).toBeInTheDocument();
    // human-readable size
    expect(screen.getByText("1.2 KB")).toBeInTheDocument();
  });

  it("renders file-type icons: csv→Table2, parquet→Database, folder→amber Folder", () => {
    const { container } = render(<FileBrowser ws="eng" conn={conn} />);
    // folder icon is the amber lucide-folder
    const folderIcon = container.querySelector(".lucide-folder");
    expect(folderIcon).toBeInTheDocument();
    expect(folderIcon).toHaveClass("text-amber-500");
    // csv → Table2
    expect(container.querySelector(".lucide-table-2")).toBeInTheDocument();
    // parquet → Database (within the table; the header tile is also a Database
    // but we just assert at least one exists)
    expect(container.querySelector(".lucide-database")).toBeInTheDocument();
  });

  it("calls refetch when Refresh is clicked", async () => {
    const user = userEvent.setup();
    render(<FileBrowser ws="eng" conn={conn} />);
    await user.click(screen.getByRole("button", { name: /refresh/i }));
    expect(refetch).toHaveBeenCalled();
  });

  it("navigates into a folder — re-queries with the new prefix", () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    // initial query at root
    expect(objectsMock).toHaveBeenCalledWith("");

    fireEvent.click(screen.getByText("sub"));

    // after navigation the hook is queried with the folder's prefix
    expect(objectsMock).toHaveBeenCalledWith("sub/");
    // breadcrumb reflects the new level
    expect(
      screen.getByRole("button", { name: /up one level/i }),
    ).toBeInTheDocument();
  });

  it("file ⋮ menu exposes Download / Move / Rename / Delete", async () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    await openMenu(/actions for readme\.txt/i);
    expect(await screen.findByText("Download")).toBeInTheDocument();
    expect(screen.getByText("Move / Rename")).toBeInTheDocument();
    expect(screen.getByText("Delete")).toBeInTheDocument();
  });

  it("folder ⋮ menu exposes Open / Move / Rename / Delete", async () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    await openMenu(/actions for sub/i);
    expect(await screen.findByText("Open")).toBeInTheDocument();
    expect(screen.getByText("Move / Rename")).toBeInTheDocument();
    expect(screen.getByText("Delete")).toBeInTheDocument();
  });

  it("calls downloadObject from the file ⋮ menu", async () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    const user = await openMenu(/actions for readme\.txt/i);
    await user.click(await screen.findByText("Download"));
    expect(downloadObject).toHaveBeenCalledWith("eng", "conn-1", "readme.txt");
  });

  it("deletes an object after confirmation via the ⋮ menu", async () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    const user = await openMenu(/actions for readme\.txt/i);
    await user.click(await screen.findByText("Delete"));

    await waitFor(() =>
      expect(screen.getByText(/Delete «readme\.txt»/)).toBeInTheDocument(),
    );

    fireEvent.change(screen.getByPlaceholderText("delete"), {
      target: { value: "delete" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^delete$/i }));

    await waitFor(() =>
      expect(deleteMutate).toHaveBeenCalledWith({ key: "readme.txt" }),
    );
  });

  it("uploads a chosen file with the current prefix in the key", async () => {
    render(<FileBrowser ws="eng" conn={conn} />);

    const input = screen.getByLabelText(/upload file/i) as HTMLInputElement;
    const file = new File(["hello"], "notes.md", { type: "text/plain" });
    // jsdom File lacks arrayBuffer in some versions — provide one.
    if (typeof file.arrayBuffer !== "function") {
      Object.defineProperty(file, "arrayBuffer", {
        value: () => Promise.resolve(new TextEncoder().encode("hello").buffer),
      });
    }

    fireEvent.change(input, { target: { files: [file] } });

    await waitFor(() =>
      expect(uploadMutate).toHaveBeenCalledWith(
        expect.objectContaining({ key: "notes.md" }),
      ),
    );
  });

  it("shows the admin notice on a 403 listing", () => {
    objectsMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 403 },
      refetch,
    });
    render(<FileBrowser ws="eng" conn={conn} />);
    expect(
      screen.getByText("Requires workspace-admin to browse storage."),
    ).toBeInTheDocument();
  });

  it("shows empty state when there are no folders or objects", () => {
    objectsMock.mockReturnValue(ok({ prefix: "", folders: [], objects: [] }));
    render(<FileBrowser ws="eng" conn={conn} />);
    expect(screen.getByText("This folder is empty")).toBeInTheDocument();
  });

  it("creates a folder at the current prefix via the new-folder dialog", async () => {
    render(<FileBrowser ws="eng" conn={conn} />);

    fireEvent.click(screen.getByRole("button", { name: /new folder/i }));

    const input = await screen.findByLabelText(/folder name/i);
    fireEvent.change(input, { target: { value: "reports" } });
    fireEvent.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() =>
      expect(createFolderMutate).toHaveBeenCalledWith(
        "reports",
        expect.anything(),
      ),
    );
  });

  it("creates a folder under a nested prefix", async () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    // navigate into "sub/"
    fireEvent.click(screen.getByText("sub"));

    fireEvent.click(screen.getByRole("button", { name: /new folder/i }));
    const input = await screen.findByLabelText(/folder name/i);
    fireEvent.change(input, { target: { value: "reports" } });
    fireEvent.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() =>
      expect(createFolderMutate).toHaveBeenCalledWith(
        "sub/reports",
        expect.anything(),
      ),
    );
  });

  it("rejects a folder name containing a slash", async () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    fireEvent.click(screen.getByRole("button", { name: /new folder/i }));
    const input = await screen.findByLabelText(/folder name/i);
    fireEvent.change(input, { target: { value: "a/b" } });
    fireEvent.click(screen.getByRole("button", { name: /^create$/i }));

    expect(createFolderMutate).not.toHaveBeenCalled();
    expect(await screen.findByText(/cannot contain/i)).toBeInTheDocument();
  });

  it("deletes a folder recursively after confirmation via the ⋮ menu", async () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    const user = await openMenu(/actions for sub/i);
    await user.click(await screen.findByText("Delete"));

    await waitFor(() =>
      expect(screen.getByText(/and ALL its contents/i)).toBeInTheDocument(),
    );

    fireEvent.change(screen.getByPlaceholderText("delete"), {
      target: { value: "delete" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^delete$/i }));

    await waitFor(() =>
      expect(deleteMutate).toHaveBeenCalledWith({
        key: "sub/",
        recursive: true,
      }),
    );
  });

  it("renames a file — destination has no trailing slash", async () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    const user = await openMenu(/actions for readme\.txt/i);
    await user.click(await screen.findByText("Move / Rename"));

    const input = await screen.findByLabelText(/new path/i);
    expect((input as HTMLInputElement).value).toBe("readme.txt");
    fireEvent.change(input, { target: { value: "notes.txt" } });
    fireEvent.click(screen.getByRole("button", { name: /^move$/i }));

    await waitFor(() =>
      expect(moveMutate).toHaveBeenCalledWith(
        { from: "readme.txt", to: "notes.txt" },
        expect.anything(),
      ),
    );
  });

  it("renames a folder — both from and to end with a slash", async () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    const user = await openMenu(/actions for sub/i);
    await user.click(await screen.findByText("Move / Rename"));

    const input = await screen.findByLabelText(/new path/i);
    expect((input as HTMLInputElement).value).toBe("sub/");
    // edit to a new name without trailing slash; component should re-add it
    fireEvent.change(input, { target: { value: "archive" } });
    fireEvent.click(screen.getByRole("button", { name: /^move$/i }));

    await waitFor(() =>
      expect(moveMutate).toHaveBeenCalledWith(
        { from: "sub/", to: "archive/" },
        expect.anything(),
      ),
    );
  });
});
