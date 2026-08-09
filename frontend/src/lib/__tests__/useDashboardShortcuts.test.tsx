import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DashboardShortcuts, isEditableTarget, type DashboardShortcutHandlers } from "../useDashboardShortcuts";

function handlers(over: Partial<DashboardShortcutHandlers> = {}): DashboardShortcutHandlers {
  return {
    editing: false,
    tabCount: 3,
    onEdit: vi.fn(), onCancelEdit: vi.fn(), onSave: vi.fn(), onToggleBookmark: vi.fn(),
    onToggleInfo: vi.fn(), onAddFilter: vi.fn(), onAddCard: vi.fn(), onSelectTab: vi.fn(),
    ...over,
  };
}

/** The hook plus the two editable surfaces the dashboard actually has: the inline name input
 *  (DashboardView.tsx) and the inline text-card textarea (TextCard.tsx:16-25). */
function Harness({ h }: { h: DashboardShortcutHandlers }) {
  return (
    <div>
      <DashboardShortcuts {...h} />
      <input aria-label="dashboard name" defaultValue="" />
      <textarea aria-label="text body" defaultValue="" />
    </div>
  );
}

describe("isEditableTarget", () => {
  it("recognises every surface the browser lets the user type into", () => {
    const input = document.createElement("input");
    const textarea = document.createElement("textarea");
    const button = document.createElement("button");
    const div = document.createElement("div");
    // jsdom does not derive isContentEditable from the attribute, so define it directly — the
    // property is what the guard reads in a real browser too.
    Object.defineProperty(div, "isContentEditable", { value: true });
    expect(isEditableTarget(input)).toBe(true);
    expect(isEditableTarget(textarea)).toBe(true);
    expect(isEditableTarget(div)).toBe(true);
    expect(isEditableTarget(button)).toBe(false);
    expect(isEditableTarget(null)).toBe(false);
  });
});

describe("dashboard shortcuts while viewing", () => {
  it("e edits, o bookmarks, ] toggles the info sidebar", async () => {
    const user = userEvent.setup();
    const h = handlers();
    render(<Harness h={h} />);
    await user.keyboard("e");
    await user.keyboard("o");
    await user.keyboard("]");
    expect(h.onEdit).toHaveBeenCalledTimes(1);
    expect(h.onCancelEdit).not.toHaveBeenCalled();
    expect(h.onToggleBookmark).toHaveBeenCalledTimes(1);
    expect(h.onToggleInfo).toHaveBeenCalledTimes(1);
  });

  it("the edit-only keys do nothing while viewing", async () => {
    const user = userEvent.setup();
    const h = handlers();
    render(<Harness h={h} />);
    await user.keyboard("sfa");
    expect(h.onSave).not.toHaveBeenCalled();
    expect(h.onAddFilter).not.toHaveBeenCalled();
    expect(h.onAddCard).not.toHaveBeenCalled();
  });
});

describe("dashboard shortcuts while editing", () => {
  it("e cancels, s saves, f adds a filter, a adds a card", async () => {
    // `e` is the one key that means two things. A handler wired to onEdit unconditionally passes
    // the viewing test above and fails here, which is why both exist.
    const user = userEvent.setup();
    const h = handlers({ editing: true });
    render(<Harness h={h} />);
    await user.keyboard("e");
    await user.keyboard("s");
    await user.keyboard("f");
    await user.keyboard("a");
    expect(h.onCancelEdit).toHaveBeenCalledTimes(1);
    expect(h.onEdit).not.toHaveBeenCalled();
    expect(h.onSave).toHaveBeenCalledTimes(1);
    expect(h.onAddFilter).toHaveBeenCalledTimes(1);
    expect(h.onAddCard).toHaveBeenCalledTimes(1);
  });

  it("the view-only keys do nothing while editing", async () => {
    const user = userEvent.setup();
    const h = handlers({ editing: true });
    render(<Harness h={h} />);
    await user.keyboard("o");
    await user.keyboard("]");
    expect(h.onToggleBookmark).not.toHaveBeenCalled();
    expect(h.onToggleInfo).not.toHaveBeenCalled();
  });
});

describe("dashboard tab shortcuts", () => {
  it("1-9 select a tab that exists and do nothing for one that does not", async () => {
    const user = userEvent.setup();
    const h = handlers({ tabCount: 3 });
    render(<Harness h={h} />);
    await user.keyboard("1");
    await user.keyboard("3");
    await user.keyboard("4");
    await user.keyboard("9");
    expect(h.onSelectTab).toHaveBeenCalledTimes(2);
    expect(h.onSelectTab).toHaveBeenNthCalledWith(1, 0);
    expect(h.onSelectTab).toHaveBeenNthCalledWith(2, 2);
  });
});

describe("shortcuts and text entry", () => {
  it("never fire while focus is in a text input or a textarea", async () => {
    // THE defect this guard exists to prevent: the dashboard has an inline name input and inline
    // heading/text card editors, so without it typing "s" in a heading saves the dashboard and
    // typing "e" leaves edit mode mid-word.
    const user = userEvent.setup();
    const h = handlers({ editing: true });
    render(<Harness h={h} />);

    const name = screen.getByLabelText("dashboard name");
    await user.click(name);
    expect(document.activeElement).toBe(name);
    await user.keyboard("sales east");
    expect(h.onSave).not.toHaveBeenCalled();
    expect(h.onAddFilter).not.toHaveBeenCalled();
    expect(h.onCancelEdit).not.toHaveBeenCalled();
    expect(name).toHaveValue("sales east");

    const body = screen.getByLabelText("text body");
    await user.click(body);
    expect(document.activeElement).toBe(body);
    await user.keyboard("a few notes");
    expect(h.onAddCard).not.toHaveBeenCalled();
    expect(body).toHaveValue("a few notes");

    // …and the very same keys still work once focus leaves the text.
    await user.click(document.body);
    await user.keyboard("s");
    expect(h.onSave).toHaveBeenCalledTimes(1);
  });

  it("leaves modifier chords to the browser", async () => {
    // Cmd/Ctrl+S must stay the browser's Save Page, not ours.
    const user = userEvent.setup();
    const h = handlers({ editing: true });
    render(<Harness h={h} />);
    await user.keyboard("{Meta>}s{/Meta}");
    await user.keyboard("{Control>}e{/Control}");
    expect(h.onSave).not.toHaveBeenCalled();
    expect(h.onCancelEdit).not.toHaveBeenCalled();
  });
});
