import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route, useLocation } from "react-router-dom";

import type { AppSummary } from "@/api/apps";
import { AppActionsMenu, OPEN_DISABLED_REASON } from "@/components/apps/AppActionsMenu";

const FOLDERS = [{ id: "f1", name: "Finance" }];

const app = (over: Partial<AppSummary>): AppSummary => ({
  id: "a1",
  name: "Revenue",
  description: "",
  updated_at: "2026-10-01T00:00:00Z",
  latest_version_no: null,
  draft_revision: 1,
  is_owner: false,
  bookmarked: false,
  folder_id: null,
  can_write: false,
  can_manage: false,
  ...over,
});

function Where() {
  return <div data-testid="where">{useLocation().pathname}</div>;
}

function handlers() {
  return {
    onRename: vi.fn(),
    onDuplicate: vi.fn(),
    onMove: vi.fn(),
    onNewFolder: vi.fn(),
    onShare: vi.fn(),
    onTrash: vi.fn(),
    onRestore: vi.fn(),
    onDeletePermanent: vi.fn(),
  };
}

function renderMenu(a: AppSummary, trashed: boolean, h = handlers()) {
  render(
    <MemoryRouter initialEntries={["/list"]}>
      <Routes>
        <Route
          path="/list"
          element={<AppActionsMenu ws="acme" app={a} trashed={trashed} folders={FOLDERS} {...h} />}
        />
        <Route path="*" element={<Where />} />
      </Routes>
    </MemoryRouter>,
  );
  return h;
}

async function openItems(): Promise<string[]> {
  await userEvent.click(screen.getByRole("button", { name: "Actions" }));
  return (await screen.findAllByRole("menuitem")).map((el) => el.textContent ?? "");
}

// The two folder moves `MoveToFolderItems` renders for an app that is in no folder.
const MOVE = ['Move to "Finance"', "Move to new folder…"];
const WRITE = ["Edit", "Rename…", "Duplicate", ...MOVE];

/**
 * Spec §4.2 as a table: for every combination of the two per-row capabilities, in and out of
 * the trash, published or not, the EXACT items — so an item leaking into a combination that
 * must not have it fails here, not only an item going missing.
 *
 * `null` means no trigger at all.
 */
const CASES: { w: boolean; m: boolean; trashed: boolean; published: boolean; want: string[] | null }[] = [];
for (const published of [false, true]) {
  CASES.push(
    { w: false, m: false, trashed: false, published, want: ["Open"] },
    { w: true, m: false, trashed: false, published, want: ["Open", ...WRITE, "Move to Trash"] },
    { w: false, m: true, trashed: false, published, want: ["Open", "Share…"] },
    { w: true, m: true, trashed: false, published, want: ["Open", ...WRITE, "Share…", "Move to Trash"] },
    { w: false, m: false, trashed: true, published, want: null },
    { w: true, m: false, trashed: true, published, want: ["Restore"] },
    { w: false, m: true, trashed: true, published, want: ["Delete permanently"] },
    { w: true, m: true, trashed: true, published, want: ["Restore", "Delete permanently"] },
  );
}

describe("AppActionsMenu — which items a row gets", () => {
  it.each(CASES)(
    "can_write=$w can_manage=$m trashed=$trashed published=$published → $want",
    async ({ w, m, trashed, published, want }) => {
      // Arrange
      renderMenu(app({ can_write: w, can_manage: m, latest_version_no: published ? 2 : null }), trashed);

      // Act + Assert
      if (want === null) {
        // Not an empty menu: no trigger, so nothing opens onto nothing.
        expect(screen.queryByRole("button", { name: "Actions" })).toBeNull();
        return;
      }
      expect(await openItems()).toEqual(want);
    },
  );
});

describe("AppActionsMenu — Open", () => {
  it("is disabled, and says why, while nothing is published", async () => {
    renderMenu(app({ can_write: true, latest_version_no: null }), false);
    await openItems();
    const open = screen.getByRole("menuitem", { name: "Open" });
    expect(open).toHaveAttribute("aria-disabled", "true");
    expect(open).toHaveAccessibleDescription(OPEN_DISABLED_REASON);
    expect(OPEN_DISABLED_REASON).toBe("Publish a draft before it can be opened.");
    expect(screen.getByText(OPEN_DISABLED_REASON)).toBeInTheDocument();
  });

  it("goes to the published view once there is one, and gives no reason", async () => {
    renderMenu(app({ latest_version_no: 3 }), false);
    await openItems();
    const open = screen.getByRole("menuitem", { name: "Open" });
    expect(open).not.toHaveAttribute("aria-disabled");
    expect(screen.queryByText(OPEN_DISABLED_REASON)).toBeNull();
    await userEvent.click(open);
    expect(screen.getByTestId("where").textContent).toBe("/explore/acme/apps/a1");
  });
});

describe("AppActionsMenu — what each item does", () => {
  it("Edit goes to the editor", async () => {
    renderMenu(app({ can_write: true }), false);
    await openItems();
    await userEvent.click(screen.getByRole("menuitem", { name: "Edit" }));
    expect(screen.getByTestId("where").textContent).toBe("/explore/acme/apps/a1/edit");
  });

  // Each item must reach ITS OWN handler — a menu wired with two items crossed passes the
  // table above untouched.
  const WIRING = [
    { item: "Rename…", handler: "onRename", trashed: false },
    { item: "Duplicate", handler: "onDuplicate", trashed: false },
    { item: "Move to new folder…", handler: "onNewFolder", trashed: false },
    { item: "Share…", handler: "onShare", trashed: false },
    { item: "Move to Trash", handler: "onTrash", trashed: false },
    { item: "Restore", handler: "onRestore", trashed: true },
    { item: "Delete permanently", handler: "onDeletePermanent", trashed: true },
  ] as const;

  it.each(WIRING)("$item calls $handler and nothing else", async ({ item, handler, trashed }) => {
    const a = app({ can_write: true, can_manage: true });
    const h = renderMenu(a, trashed);
    await openItems();
    await userEvent.click(screen.getByRole("menuitem", { name: item }));
    expect(h[handler]).toHaveBeenCalledWith(a);
    for (const [name, fn] of Object.entries(h)) {
      if (name !== handler) expect(fn).not.toHaveBeenCalled();
    }
  });

  it("moving to a folder reports the app and the folder id", async () => {
    const a = app({ can_write: true });
    const h = renderMenu(a, false);
    await openItems();
    await userEvent.click(screen.getByRole("menuitem", { name: 'Move to "Finance"' }));
    expect(h.onMove).toHaveBeenCalledWith(a, "f1");
  });
});
