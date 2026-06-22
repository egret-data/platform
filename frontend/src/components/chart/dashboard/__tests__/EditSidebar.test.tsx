import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { EditSidebar } from "../EditSidebar";

const noop = vi.fn();
const common = { charts: [], parameters: [], tabs: [], cardForPanel: undefined, chartForCard: undefined, paramForPanel: undefined,
  onAddChart: noop, onAddVirtual: noop, onUpdateCardOptions: noop, onMoveCardToTab: noop, onChangeParam: noop, onDeleteParam: noop, onClose: noop };

describe("EditSidebar", () => {
  it("renders the Add panel for kind 'add'", () => {
    render(<EditSidebar panel={{ kind: "add" }} {...common} />);
    expect(screen.getByLabelText("search charts")).toBeTruthy();
  });
  it("renders the Filter panel for kind 'filter'", () => {
    const param = { id: "a", name: "Region", slug: "region", type: "category", operator: "is" } as never;
    render(<EditSidebar panel={{ kind: "filter", paramId: "a" }} {...common} paramForPanel={param} />);
    expect(screen.getByLabelText("parameter name")).toBeTruthy();
  });
});
