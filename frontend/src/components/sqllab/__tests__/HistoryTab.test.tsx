import { render, screen, fireEvent } from "@testing-library/react";
import { expect, test, vi, beforeEach } from "vitest";

const historyData = [
  { id: "h1", database_id: "db1", status: "finished", sql: "SELECT 42", row_count: 1, submitted_at: new Date(Date.now() - 60_000).toISOString() },
  { id: "h2", database_id: "db1", status: "failed", sql: "SELECT boom", row_count: null, submitted_at: new Date(Date.now() - 120_000).toISOString() },
];
vi.mock("@/api/sqllab", () => ({ useHistory: () => ({ data: historyData }) }));

import { HistoryTab } from "@/components/sqllab/HistoryTab";

const databases = [{ id: "db1", name: "trino-local", engine: "trino" }];

beforeEach(() => {
  Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });
});

test("renders status badge, db name, and sql for each row", () => {
  render(<HistoryTab ws="eng" databases={databases} onLoad={vi.fn()} onRun={vi.fn()} />);
  expect(screen.getByText("finished")).toBeInTheDocument();
  expect(screen.getByText("failed")).toBeInTheDocument();
  expect(screen.getAllByText("trino-local").length).toBe(2);
  expect(screen.getByText("SELECT 42")).toBeInTheDocument();
});

test("clicking a row loads its sql", () => {
  const onLoad = vi.fn();
  render(<HistoryTab ws="eng" databases={databases} onLoad={onLoad} onRun={vi.fn()} />);
  fireEvent.click(screen.getByText("SELECT 42"));
  expect(onLoad).toHaveBeenCalledWith("SELECT 42");
});

test("re-run calls onRun with sql and db id", () => {
  const onRun = vi.fn();
  render(<HistoryTab ws="eng" databases={databases} onLoad={vi.fn()} onRun={onRun} />);
  fireEvent.click(screen.getAllByRole("button", { name: "Re-run" })[0]);
  expect(onRun).toHaveBeenCalledWith("SELECT 42", "db1");
});

test("copy writes the sql to the clipboard", () => {
  render(<HistoryTab ws="eng" databases={databases} onLoad={vi.fn()} onRun={vi.fn()} />);
  fireEvent.click(screen.getAllByRole("button", { name: "Copy SQL" })[0]);
  expect(navigator.clipboard.writeText).toHaveBeenCalledWith("SELECT 42");
});
