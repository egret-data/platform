import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { FileRow } from "@/components/workflows/executions/files/FileRow";
import { formatBytes } from "@/components/workflows/executions/files/fileModel";
import { EXEC, FileServer, uri, WS } from "./fileServer";

/**
 * A file row and its preview drawer, through the real orval hooks and the
 * file routes served from memory (FE-TEST-1).
 */
function renderRow(path: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <FileRow ws={WS} executionId={EXEC} uri={path} />
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("formatBytes", () => {
  it("writes bytes, then binary units with one decimal", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1023)).toBe("1023 B");
    expect(formatBytes(1024)).toBe("1.0 KiB");
    expect(formatBytes(1536)).toBe("1.5 KiB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MiB");
    expect(formatBytes(3 * 1024 ** 3)).toBe("3.0 GiB");
  });
});

describe("FileRow", () => {
  it("shows the name, the size read from file/metas, Preview, and Download as a link to the file route", async () => {
    const path = uri("orders.csv");
    const server = new FileServer().file(path, 2560).install();
    renderRow(path);

    expect(screen.getByText("orders.csv")).toHaveAttribute("title", path);
    expect(await screen.findByText("2.5 KiB")).toBeInTheDocument();
    // The metas read names the file by its URI.
    expect(server.callsTo("metas").map((c) => c.query.get("path"))).toEqual([path]);

    const download = screen.getByRole("link", { name: "Download orders.csv" });
    // A plain link, so the browser streams it with the session cookie (no fetch).
    const href = new URL(download.getAttribute("href") ?? "", "http://x");
    expect(href.pathname).toBe(`/api/ws/${WS}/workflows/executions/${EXEC}/file`);
    expect(href.searchParams.get("path")).toBe(path);
    expect(screen.getByRole("button", { name: "Preview orders.csv" })).toBeEnabled();
  });

  it("says a file the route cannot find is no longer available, with nothing to preview or download", async () => {
    new FileServer().install();
    renderRow(uri("gone.csv"));
    expect(await screen.findByText("This file is no longer available")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Preview/ })).toBeNull();
    expect(screen.queryByRole("link", { name: /Download/ })).toBeNull();
  });

  it("keeps Preview and Download when only the size could not be read, and says so", async () => {
    const path = uri("orders.csv");
    const server = new FileServer().install();
    server.metas.set(path, { status: 500, body: { error: "storage unreachable" } });
    renderRow(path);
    expect(await screen.findByText(/The size could not be read/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Preview orders.csv" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Download orders.csv" })).toBeInTheDocument();
  });
});

describe("FilePreviewDrawer", () => {
  async function openPreview(name: string) {
    await userEvent.click(await screen.findByRole("button", { name: `Preview ${name}` }));
    return screen.findByRole("dialog", { name: `Preview of ${name}` });
  }

  it("shows an Ion file's rows as JSON, one block per value", async () => {
    const path = uri("rows.ion");
    new FileServer()
      .file(path, 100, { previewable: true, truncated: false, rows: [{ id: 1, name: "a" }, { id: 2, tags: ["x"] }] })
      .install();
    renderRow(path);
    const drawer = await openPreview("rows.ion");
    const rows = await within(drawer).findAllByTestId("preview-row");
    expect(rows.map((r) => JSON.parse(r.textContent ?? ""))).toEqual([{ id: 1, name: "a" }, { id: 2, tags: ["x"] }]);
    // Pretty-printed, not the one-line form.
    expect(rows[0].textContent).toContain('\n  "id": 1');
  });

  it("shows a text file line by line, as text and not as JSON strings", async () => {
    const path = uri("log.txt");
    new FileServer().file(path, 100, { previewable: true, truncated: false, rows: ["first line", '{"not": "parsed"}'] }).install();
    renderRow(path);
    const drawer = await openPreview("log.txt");
    const rows = await within(drawer).findAllByTestId("preview-row");
    expect(rows.map((r) => r.textContent)).toEqual(["first line", '{"not": "parsed"}']);
  });

  it("says a file that is neither text nor Ion cannot be previewed, and offers the download", async () => {
    const path = uri("photo.png");
    new FileServer().file(path, 100, { previewable: false, truncated: false, rows: [] }).install();
    renderRow(path);
    const drawer = await openPreview("photo.png");
    expect(await within(drawer).findByText("This file cannot be previewed")).toBeInTheDocument();
    expect(within(drawer).queryAllByTestId("preview-row")).toHaveLength(0);
    expect(within(drawer).getByRole("link", { name: /Download/ })).toBeInTheDocument();
  });

  it("asks for 100 rows, says a truncated preview is truncated with the row cap, and asks again with a larger cap", async () => {
    const path = uri("big.txt");
    const server = new FileServer()
      .file(path, 10_000, (n) => ({ previewable: true, truncated: true, rows: Array.from({ length: n ?? 100 }, (_, i) => `line ${i}`) }))
      .install();
    renderRow(path);
    const drawer = await openPreview("big.txt");
    expect(await within(drawer).findByText(/Showing the first 100 rows/)).toBeInTheDocument();
    expect(within(drawer).getAllByTestId("preview-row")).toHaveLength(100);
    expect(server.callsTo("preview").map((c) => c.query.get("maxRows"))).toEqual(["100"]);

    await userEvent.click(within(drawer).getByRole("combobox", { name: "Rows" }));
    await userEvent.click(await screen.findByRole("option", { name: "500" }));
    expect(await within(drawer).findByText(/Showing the first 500 rows/)).toBeInTheDocument();
    expect(within(drawer).getAllByTestId("preview-row")).toHaveLength(500);
    expect(server.callsTo("preview").map((c) => c.query.get("maxRows"))).toEqual(["100", "500"]);
  });

  it("says when the preview's size limit, not the row cap, cut it short", async () => {
    const path = uri("wide.txt");
    new FileServer().file(path, 10_000_000, { previewable: true, truncated: true, rows: ["a", "b", "c"] }).install();
    renderRow(path);
    const drawer = await openPreview("wide.txt");
    expect(await within(drawer).findByText(/Showing the first 3 rows: the preview's size limit/)).toBeInTheDocument();
  });

  it("does not call a complete preview truncated", async () => {
    const path = uri("small.txt");
    new FileServer().file(path, 10, { previewable: true, truncated: false, rows: ["only"] }).install();
    renderRow(path);
    const drawer = await openPreview("small.txt");
    await within(drawer).findByText("only");
    expect(within(drawer).queryByText(/Showing the first/)).toBeNull();
  });

  it("says a file the preview cannot find is no longer available", async () => {
    const path = uri("pruned.txt");
    const server = new FileServer().file(path, 10).install();
    // The size was read, then the file was pruned before the preview.
    renderRow(path);
    const drawer = await openPreview("pruned.txt");
    expect(await within(drawer).findByText("This file is no longer available")).toBeInTheDocument();
    expect(server.callsTo("preview")).toHaveLength(1);
  });

  it("shows another failure's message with a retry, not as missing", async () => {
    const path = uri("flaky.txt");
    const server = new FileServer().file(path, 10).install();
    server.previews.set(path, () => ({ status: 503, body: { error: "storage is busy" } }));
    renderRow(path);
    const drawer = await openPreview("flaky.txt");
    expect(await within(drawer).findByText(/storage is busy/)).toBeInTheDocument();
    expect(within(drawer).queryByText("This file is no longer available")).toBeNull();
    server.previews.set(path, () => ({ status: 200, body: { previewable: true, truncated: false, rows: ["back"] } }));
    await userEvent.click(within(drawer).getByRole("button", { name: "Retry" }));
    expect(await within(drawer).findByText("back")).toBeInTheDocument();
  });

  it("closes on Escape and gives focus back to the Preview button", async () => {
    const path = uri("log.txt");
    new FileServer().file(path, 100, { previewable: true, truncated: false, rows: ["x"] }).install();
    renderRow(path);
    const drawer = await openPreview("log.txt");
    await within(drawer).findByText("x");
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("button", { name: "Preview log.txt" })).toHaveFocus();
  });
});
