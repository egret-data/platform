import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, test, vi } from "vitest";

const updateColumn = vi.hoisted(() => vi.fn());

// Mock @/api/datasets so no real fetch is made; mutations are no-ops.
vi.mock("@/api/datasets", () => ({
  useDatasetMutations: () => ({
    addColumn: { mutateAsync: vi.fn(), isPending: false },
    updateColumn: { mutateAsync: updateColumn, isPending: false },
    deleteColumn: { mutateAsync: vi.fn(), isPending: false },
    addMetric: { mutateAsync: vi.fn(), isPending: false },
    updateMetric: { mutateAsync: vi.fn(), isPending: false },
    deleteMetric: { mutateAsync: vi.fn(), isPending: false },
  }),
  useSetDatasetCertification: () => ({ mutateAsync: vi.fn(), isPending: false }),
}));
// MetricsTab reads the session to decide whether to offer the certify switch (0109). These
// tests are about rendering rows, so grant it: a steward sees the fullest markup, which is the
// stricter thing to assert against.
vi.mock("@/auth/useAuth", () => ({
  useAuth: () => ({ me: { platform_roles: [], permissions: ["dataset:certify"] } }),
}));

import { ColumnsTab } from "@/components/dataset/ColumnsTab";
import { MetricsTab } from "@/components/dataset/MetricsTab";
import type { DatasetColumn, DatasetMetric } from "@/api/datasets";

function renderWithProviders(ui: React.ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        {ui}
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const physicalColumn: DatasetColumn = {
  column_name: "order_id",
  data_type: "bigint",
  type_generic: "numeric",
  is_temporal: false,
  ordinal: 1,
  expression: null,
  verbose_name: "",
  description: "",
  is_calculated: false,
  groupby: true,
  filterable: true,
  format: "",
  semantic_type: "",
};

const calculatedColumn: DatasetColumn = {
  column_name: "revenue",
  data_type: "double",
  type_generic: "numeric",
  is_temporal: false,
  ordinal: 2,
  expression: "price * quantity",
  verbose_name: "Revenue",
  description: "Total revenue",
  is_calculated: true,
  groupby: true,
  filterable: true,
  format: "",
  semantic_type: "",
};

const sampleMetric: DatasetMetric = {
  id: "m1",
  metric_name: "total_revenue",
  expression: "SUM(revenue)",
  data_type: "double",
  type_generic: "numeric",
  verbose_name: "Total Revenue",
  description: "Sum of revenue",
  ordinal: 1,
  format: "",
  certified: false,
  certified_by: "",
  certification_details: "",
};

test("ColumnsTab: calculated column shows 'calculated' badge", () => {
  renderWithProviders(
    <ColumnsTab ws="eng" datasetId="ds1" columns={[physicalColumn, calculatedColumn]} />,
  );
  expect(screen.getByText("calculated")).toBeInTheDocument();
});

test("ColumnsTab: physical column does NOT show 'calculated' badge", () => {
  renderWithProviders(
    <ColumnsTab ws="eng" datasetId="ds1" columns={[physicalColumn, calculatedColumn]} />,
  );
  // There should be exactly one "calculated" badge (only for calculatedColumn)
  expect(screen.getAllByText("calculated")).toHaveLength(1);
});

test("ColumnsTab: '+ Calculated column' button renders", () => {
  renderWithProviders(
    <ColumnsTab ws="eng" datasetId="ds1" columns={[physicalColumn, calculatedColumn]} />,
  );
  expect(screen.getByRole("button", { name: "+ Calculated column" })).toBeInTheDocument();
});

/**
 * THE ROLE SELECT'S VOCABULARY, PINNED.
 *
 * `semantic_type` is now validated server-side against `SEMANTIC_TYPES` in `egret-analytics-api`,
 * which answers 400 for anything else. That makes this picker and that Rust constant two halves
 * of one contract, and the failure mode of them drifting is specific and bad: add an option
 * here without adding it there and the control offers a choice the API refuses, so the user picks a
 * role and gets an error toast from a dropdown that suggested it.
 *
 * The Rust side has its own test pinning the constant. That one CANNOT see this file — it compares
 * the constant against a written-down copy of this list — so it catches the constant changing alone.
 * This test is the other direction, and the two together are what make the pair hard to break: this
 * one fails the moment an option is added, removed or respelled here, and its message says where
 * the matching change belongs. Neither test alone is enough, which is why the Rust comment now
 * points at both.
 *
 * The values, not the labels: the API compares the `value`, and every reader (`map.tsx`,
 * `PreviewTab`) compares it with `===`. A label may be re-worded freely.
 */
test("ColumnsTab: the Role select offers exactly the server's semantic_type vocabulary", async () => {
  const user = userEvent.setup();
  updateColumn.mockReset();
  updateColumn.mockResolvedValue(undefined);
  // The picker does not put values in the DOM, so each option is picked and the value it SAVES is
  // read off the mutation. A pick of the option already selected saves nothing, so every option is
  // picked from a row whose role is a different one.
  const labels = ["—", "Category", "Country", "Currency", "Email", "URL", "Image", "Latitude", "Longitude"];
  const saved: unknown[] = [];
  for (const label of labels) {
    const start = label === "Latitude" ? "longitude" : "latitude";
    const { unmount } = renderWithProviders(
      <ColumnsTab ws="eng" datasetId="ds1" columns={[{ ...physicalColumn, semantic_type: start }]} />,
    );
    await user.click(screen.getByRole("combobox", { name: `Role ${physicalColumn.column_name}` }));
    const offered = screen.getAllByRole("option").map((o) => o.textContent);
    expect(offered).toStrictEqual(labels);
    await user.click(screen.getByRole("option", { name: label }));
    saved.push((updateColumn.mock.lastCall?.[0] as { body: { semantic_type: string } }).body.semantic_type);
    unmount();
  }
  expect(saved).toStrictEqual([
    "", "category", "country", "currency", "email", "url", "image", "latitude", "longitude",
  ]);
  // If this failed, update `SEMANTIC_TYPES` in crates/egret-analytics-api/src/bi.rs, the literal in
  // its `the_allow_list_is_exactly_the_editor_vocabulary` test, and migration 0104's CHECK — all
  // three, or the new value is a 400 the moment someone picks it.
  // Nine render-open-pick rounds: past the 5s default on a loaded machine.
}, 20_000);

test("MetricsTab: metric row renders with correct metric_name", () => {
  renderWithProviders(
    <MetricsTab ws="eng" datasetId="ds1" metrics={[sampleMetric]} />,
  );
  expect(screen.getByText("total_revenue")).toBeInTheDocument();
});

test("MetricsTab: '+ Metric' button renders", () => {
  renderWithProviders(
    <MetricsTab ws="eng" datasetId="ds1" metrics={[sampleMetric]} />,
  );
  expect(screen.getByRole("button", { name: "+ Metric" })).toBeInTheDocument();
});

test("MetricsTab: empty state message renders when no metrics", () => {
  renderWithProviders(
    <MetricsTab ws="eng" datasetId="ds1" metrics={[]} />,
  );
  expect(screen.getByText("No metrics yet.")).toBeInTheDocument();
});
