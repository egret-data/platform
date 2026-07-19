import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { NotebookHeader } from "../NotebookHeader";

function renderHeader(overrides: Partial<React.ComponentProps<typeof NotebookHeader>> = {}) {
  const props: React.ComponentProps<typeof NotebookHeader> = {
    ws: "demo",
    name: "My NB",
    isPublic: false,
    status: "idle",
    runningAll: false,
    activePanel: null,
    onTogglePanel: vi.fn(),
    onRename: vi.fn(),
    onTogglePublic: vi.fn(),
    onRunAll: vi.fn(),
    onInterrupt: vi.fn(),
    onRestart: vi.fn(),
    onClear: vi.fn(),
    ...overrides,
  };
  return { props, ...render(<MemoryRouter><NotebookHeader {...props} /></MemoryRouter>) };
}

describe("NotebookHeader", () => {
  it("shows the kernel status and fires Run all", async () => {
    const { props } = renderHeader();
    expect(screen.getByText("Ready")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /run all/i }));
    expect(props.onRunAll).toHaveBeenCalled();
  });
});
