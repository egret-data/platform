import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BulkMoveToFolderMenu, moveEach } from "../BulkMoveToFolderMenu";

const FOLDERS = [{ id: "f1", name: "Finance" }, { id: "f2", name: "Ops" }];
const items = () => screen.getAllByRole("menuitem").map((el) => el.textContent);
const open = (count: number) => userEvent.click(screen.getByRole("button", { name: `Move ${count} to folder…` }));

describe("BulkMoveToFolderMenu", () => {
  it("lists the folders that exist, then Uncategorized, then New folder…", async () => {
    // Arrange
    render(<BulkMoveToFolderMenu count={3} folders={FOLDERS} onMove={vi.fn()} onNewFolder={vi.fn()} />);

    // Act
    await open(3);

    // Assert
    expect(items()).toEqual(["Finance", "Ops", "Uncategorized", "New folder…"]);
  });

  it("hands the chosen folder's id to onMove, and null for Uncategorized", async () => {
    // Arrange
    const onMove = vi.fn();
    render(<BulkMoveToFolderMenu count={2} folders={FOLDERS} onMove={onMove} onNewFolder={vi.fn()} />);

    // Act
    await open(2);
    await userEvent.click(screen.getByRole("menuitem", { name: "Ops" }));
    await open(2);
    await userEvent.click(screen.getByRole("menuitem", { name: "Uncategorized" }));

    // Assert
    expect(onMove.mock.calls).toEqual([["f2"], [null]]);
  });

  it("withholds New folder… from a caller who may not create one", async () => {
    // Arrange
    render(<BulkMoveToFolderMenu count={1} folders={FOLDERS} onMove={vi.fn()} />);

    // Act
    await open(1);

    // Assert
    expect(items()).toEqual(["Finance", "Ops", "Uncategorized"]);
  });

  it("offers only New folder… when there is no folder yet", async () => {
    // Arrange
    const onNewFolder = vi.fn();
    render(<BulkMoveToFolderMenu count={1} folders={[]} onMove={vi.fn()} onNewFolder={onNewFolder} />);

    // Act
    await open(1);

    // Assert: no Uncategorized either — with no folders nothing is IN one
    expect(items()).toEqual(["New folder…"]);
    await userEvent.click(screen.getByRole("menuitem", { name: "New folder…" }));
    expect(onNewFolder).toHaveBeenCalledTimes(1);
  });

  it("draws nothing when there is no folder and none can be made", () => {
    // Arrange / Act
    const { container } = render(<BulkMoveToFolderMenu count={1} folders={[]} onMove={vi.fn()} />);

    // Assert
    expect(container.firstChild).toBeNull();
  });
});

describe("moveEach", () => {
  it("moves every id and reports how many", async () => {
    // Arrange
    const move = vi.fn().mockResolvedValue(undefined);
    const onError = vi.fn();

    // Act
    const moved = await moveEach(["a", "b", "c"], move, onError);

    // Assert
    expect(move.mock.calls.map((c) => c[0])).toEqual(["a", "b", "c"]);
    expect(moved).toBe(3);
    expect(onError).not.toHaveBeenCalled();
  });

  it("keeps going past a refusal, reports it, and does not count it", async () => {
    // Arrange: the old loop stopped at the first error and left the rest unmoved
    const refusal = new Error("forbidden");
    const move = vi.fn().mockResolvedValueOnce(undefined).mockRejectedValueOnce(refusal).mockResolvedValueOnce(undefined);
    const onError = vi.fn();

    // Act
    const moved = await moveEach(["a", "b", "c"], move, onError);

    // Assert
    expect(move).toHaveBeenCalledTimes(3);
    expect(moved).toBe(2);
    expect(onError).toHaveBeenCalledWith(refusal);
  });
});
