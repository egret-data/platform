import type { DraftCard } from "./tabs";
import { GRID_COLS } from "@/components/chart/dashboard-grid";

// Clean-room port of Metabase's dashboard sections (frontend/src/metabase/dashboard/sections.ts),
// expressed in egret's 24-col grid model. Each section emits a full-width heading plus
// placeholder slots the user clicks to fill with a chart.
export type SectionId = "kpi_grid" | "large_chart_kpi_right" | "kpi_chart_below";

export interface SectionDraftCard {
  card_type: "heading" | "placeholder";
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
}

export interface SectionLayout {
  id: SectionId;
  label: string;
  build: (origin: { col: number; row: number }) => SectionDraftCard[];
}

const HEADING_HEIGHT = 1;
const SCALAR_W = 6;
const SCALAR_H = 3;

function heading({ col, row }: { col: number; row: number }): SectionDraftCard {
  return { card_type: "heading", grid_col: col, grid_row: row, size_x: GRID_COLS, size_y: HEADING_HEIGHT };
}

function ph(grid_col: number, grid_row: number, size_x: number, size_y: number): SectionDraftCard {
  return { card_type: "placeholder", grid_col, grid_row, size_x, size_y };
}

// 2x2 grid of half-width KPI placeholders (Metabase getKpiGridLayout: scalarCardHeight = 5).
function buildKpiGrid({ col, row }: { col: number; row: number }): SectionDraftCard[] {
  const r = row + HEADING_HEIGHT;
  const w = GRID_COLS / 2; // 12
  const h = 5;
  return [
    heading({ col, row }),
    ph(col, r, w, h), ph(col + w, r, w, h),
    ph(col, r + h, w, h), ph(col + w, r + h, w, h),
  ];
}

// Large chart with a column of three KPI placeholders on the right (getLargeChartKpiColLayout).
function buildLargeChartKpiRight({ col, row }: { col: number; row: number }): SectionDraftCard[] {
  const r = row + HEADING_HEIGHT;
  const largeW = GRID_COLS - SCALAR_W; // 18
  const right = [
    ph(col + largeW, r, SCALAR_W, SCALAR_H),
    ph(col + largeW, r + SCALAR_H, SCALAR_W, SCALAR_H),
    ph(col + largeW, r + SCALAR_H * 2, SCALAR_W, SCALAR_H),
  ];
  const large = ph(col, r, largeW, SCALAR_H * right.length); // 18 x 9
  return [heading({ col, row }), large, ...right];
}

// Row of four KPI placeholders, then a full-width large chart below (getKpiLargeChartBelowLayout).
function buildKpiChartBelow({ col, row }: { col: number; row: number }): SectionDraftCard[] {
  const r = row + HEADING_HEIGHT;
  const topRow = [
    ph(col, r, SCALAR_W, SCALAR_H),
    ph(col + SCALAR_W, r, SCALAR_W, SCALAR_H),
    ph(col + SCALAR_W * 2, r, SCALAR_W, SCALAR_H),
    ph(col + SCALAR_W * 3, r, SCALAR_W, SCALAR_H),
  ];
  const large = ph(col, r + SCALAR_H, GRID_COLS, 9);
  return [heading({ col, row }), ...topRow, large];
}

export const layoutOptions: SectionLayout[] = [
  { id: "kpi_grid", label: "KPI grid", build: buildKpiGrid },
  { id: "large_chart_kpi_right", label: "Large chart with KPIs on the right", build: buildLargeChartKpiRight },
  { id: "kpi_chart_below", label: "KPIs with large chart below", build: buildKpiChartBelow },
];

/** Unfilled placeholder cards are edit-only and must be removed before persisting to the API. */
export function dropPlaceholders(cards: DraftCard[]): DraftCard[] {
  return cards.filter((c) => c.card_type !== "placeholder");
}
