import { afterEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import WorkflowsRoutes from "@/routes/workflows/WorkflowsRoutes";
import { NS_WS, nsRow, serveNamespaces } from "./fakeNamespaceServer";

/**
 * The Files tab holds unsaved edits in its open editors. Radix unmounts an
 * inactive tab's content, so a visit to KV and back used to drop them. The
 * stub stands in for the explorer with what matters here: state that lives
 * only as long as the component is mounted (an edit, and its unsaved dot).
 */
vi.mock("@/components/workflows/namespaces/NamespaceFilesTab", () => ({
  default: function FilesStub() {
    const [text, setText] = useState("saved");
    return (
      <section data-testid="namespace-tab-files">
        <label>
          Contents of a.py
          <textarea value={text} onChange={(e) => setText(e.target.value)} />
        </label>
        {text !== "saved" ? <span data-testid="unsaved-dot">unsaved</span> : null}
      </section>
    );
  },
}));

const LAZY = { timeout: 4000 };
const at = (tab: string) => `/explore/${NS_WS}/workflows/namespaces/company.team/${tab}`;

function Where() {
  const l = useLocation();
  return <output data-testid="where">{l.pathname}</output>;
}

function renderAt(path: string) {
  serveNamespaces({ namespaces: [nsRow("company.team", { files: 1 })], flows: [], executions: [] });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } } });
  render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[path]}>
          <Routes>
            <Route
              path="/explore/:workspace/workflows/*"
              element={
                <>
                  <WorkflowsRoutes />
                  <Where />
                </>
              }
            />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => vi.unstubAllGlobals());

describe("NamespaceDetail: unsaved file edits across tabs", () => {
  it("keeps the Files tab's edits and unsaved dot after a visit to KV", async () => {
    const user = userEvent.setup();
    renderAt(at("files"));
    const editor = await screen.findByRole("textbox", { name: "Contents of a.py" }, LAZY);
    await user.type(editor, " and edited");
    expect(screen.getByTestId("unsaved-dot")).toBeInTheDocument();

    const list = screen.getByRole("tablist");
    await user.click(within(list).getByRole("tab", { name: "KV" }));
    expect(await screen.findByText("No KV keys yet", {}, LAZY)).toBeInTheDocument();
    // Hidden, not shown beside KV.
    expect(screen.queryByRole("textbox", { name: "Contents of a.py" })).not.toBeInTheDocument();
    await user.click(within(list).getByRole("tab", { name: "Files" }));

    expect(screen.getByRole("textbox", { name: "Contents of a.py" })).toHaveValue("saved and edited");
    expect(screen.getByTestId("unsaved-dot")).toBeInTheDocument();
  });

  it("does not mount Files before it is visited", async () => {
    renderAt(at("kv"));
    expect(await screen.findByText("No KV keys yet", {}, LAZY)).toBeInTheDocument();
    expect(screen.queryByTestId("namespace-tab-files")).not.toBeInTheDocument();
  });

  it("moves focus with the arrows without selecting (manual activation)", async () => {
    const user = userEvent.setup();
    renderAt(at("overview"));
    const list = await screen.findByRole("tablist", {}, LAZY);
    within(list).getByRole("tab", { name: "Overview" }).focus();
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(within(list).getByRole("tab", { name: "Flows" })).toHaveFocus());
    // Let a navigation the focus would cause land before asserting there was none.
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(screen.getByTestId("where").textContent).toBe(at("overview"));
    await user.keyboard("{Enter}");
    expect(screen.getByTestId("where").textContent).toBe(at("flows"));
  });
});
