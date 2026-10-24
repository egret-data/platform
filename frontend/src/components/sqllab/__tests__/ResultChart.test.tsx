import { render } from "@testing-library/react";
import { expect, test } from "vitest";
import { ResultChart } from "@/components/sqllab/ResultChart";
import type { ResultsPage } from "@/api/sqllab";

const page = (rows: unknown[][], yType = "bigint"): ResultsPage => ({
  columns: [{ name: "orderstatus", type: "varchar" }, { name: "orders", type: yType }],
  rows, page: 0, page_size: 1000, row_count: rows.length, truncated: false,
});

const yTicks = (c: HTMLElement) =>
  [...c.querySelectorAll("svg text[text-anchor='end']:not([transform])")].map((t) => t.textContent ?? "");
const xLabels = (c: HTMLElement) =>
  [...c.querySelectorAll("svg text")].filter((t) => ["F", "O", "P"].includes(t.textContent ?? ""));

/** KI-162: counts get whole-number ticks, and short labels stay upright. */
test("an integer measure gets whole-number ticks and one-letter labels are not rotated", () => {
  const { container } = render(<ResultChart page={page([["F", 7304], ["O", 7333], ["P", 363]])} />);
  const ticks = yTicks(container);
  expect(ticks.length).toBeGreaterThan(1);
  for (const t of ticks) expect(t).toMatch(/^-?[\d,]+$/);
  const labels = xLabels(container);
  expect(labels).toHaveLength(3);
  for (const l of labels) expect(l.getAttribute("transform")).toBeNull();
});

test("ticks are round steps that cover the largest value", () => {
  const { container } = render(<ResultChart page={page([["F", 7304], ["O", 7333], ["P", 363]])} />);
  const nums = yTicks(container).map((t) => Number(t.replace(/,/g, "")));
  expect(nums[0]).toBe(0);
  expect(Math.max(...nums)).toBeGreaterThanOrEqual(7333);
  const step = nums[1] - nums[0];
  expect(String(step)).toMatch(/^(1|2|5)0*$/);
});

test("a fractional measure keeps fractional ticks", () => {
  const { container } = render(<ResultChart page={page([["F", 0.25], ["O", 0.5], ["P", 0.75]], "double")} />);
  expect(yTicks(container).some((t) => t.includes("."))).toBe(true);
});

test("long labels that do not fit their band are still rotated", () => {
  const rows = Array.from({ length: 30 }, (_, i) => [`customer-segment-${i}`, i + 1]);
  const { container } = render(<ResultChart page={page(rows)} />);
  const rotated = [...container.querySelectorAll("svg text[transform]")];
  expect(rotated.length).toBe(30);
});
