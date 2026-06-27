import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

// ── Mock factories ────────────────────────────────────────────────────────────
const objectsMock = vi.fn();
const uploadMutation = vi.fn();
const uploadMutate = vi.fn();
const deleteMutation = vi.fn();
const deleteMutate = vi.fn();
const downloadObject = vi.fn();

vi.mock("@/api/storage", () => ({
  useStorageObjects: (_ws: string, _id: string, prefix: string) =>
    objectsMock(prefix),
  useUploadObject: () => uploadMutation(),
  useDeleteObject: () => deleteMutation(),
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
  ],
};

function ok(data: typeof listing) {
  return { data, isLoading: false, isError: false, error: null };
}

beforeEach(() => {
  objectsMock.mockReset();
  uploadMutation.mockReset();
  uploadMutate.mockReset();
  deleteMutation.mockReset();
  deleteMutate.mockReset();
  downloadObject.mockReset();

  objectsMock.mockReturnValue(ok(listing));
  uploadMutation.mockReturnValue({ mutate: uploadMutate, isPending: false });
  deleteMutation.mockReturnValue({ mutate: deleteMutate, isPending: false });
  downloadObject.mockResolvedValue(undefined);
});

describe("FileBrowser", () => {
  it("renders folders and files from the listing", () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    expect(screen.getByText("sub")).toBeInTheDocument();
    expect(screen.getByText("readme.txt")).toBeInTheDocument();
    // human-readable size
    expect(screen.getByText("1.2 KB")).toBeInTheDocument();
  });

  it("navigates into a folder — re-queries with the new prefix", () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    // initial query at root
    expect(objectsMock).toHaveBeenCalledWith("");

    fireEvent.click(screen.getByText("sub"));

    // after navigation the hook is queried with the folder's prefix
    expect(objectsMock).toHaveBeenCalledWith("sub/");
    // breadcrumb reflects the new level
    expect(screen.getByRole("button", { name: /up one level/i })).toBeInTheDocument();
  });

  it("calls downloadObject when Download is clicked", () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    fireEvent.click(screen.getByRole("button", { name: /download readme\.txt/i }));
    expect(downloadObject).toHaveBeenCalledWith("eng", "conn-1", "readme.txt");
  });

  it("deletes an object after confirmation", async () => {
    render(<FileBrowser ws="eng" conn={conn} />);
    fireEvent.click(screen.getByRole("button", { name: /delete readme\.txt/i }));

    await waitFor(() =>
      expect(screen.getByText(/Delete «readme\.txt»/)).toBeInTheDocument(),
    );

    fireEvent.change(screen.getByPlaceholderText("delete"), {
      target: { value: "delete" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^delete$/i }));

    await waitFor(() => expect(deleteMutate).toHaveBeenCalledWith("readme.txt"));
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
    });
    render(<FileBrowser ws="eng" conn={conn} />);
    expect(
      screen.getByText("Requires workspace-admin to browse storage."),
    ).toBeInTheDocument();
  });

  it("shows empty state when there are no folders or objects", () => {
    objectsMock.mockReturnValue(ok({ prefix: "", folders: [], objects: [] }));
    render(<FileBrowser ws="eng" conn={conn} />);
    expect(screen.getByText("Empty")).toBeInTheDocument();
  });
});
