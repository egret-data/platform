import { sectionCounts } from "../useEngineConfigForm";
import type { TrinoConfig } from "@/api/controlPlane";

test("counts non-default field-groups per section", () => {
  const cfg: TrinoConfig = {
    coordinator: { replicas: 2 },
    worker: {},
    autoscaling: { mode: "hpa", hpa: { maxReplicas: 5 } },
  };
  const counts = sectionCounts(cfg);
  expect(counts.coordinator).toBeGreaterThanOrEqual(1);
  expect(counts.autoscaling).toBeGreaterThanOrEqual(1);
  expect(counts.general).toBe(0);
  expect(counts.worker).toBe(0);
  expect(counts.placement).toBe(0);
});

test("per-role scheduling fields count under placement only", () => {
  const cfg: TrinoConfig = {
    coordinator: { nodeSelector: { disk: "ssd" }, resources: { cpuLimit: "4" } },
    worker: {},
  };
  const counts = sectionCounts(cfg);
  // resources counts under coordinator, nodeSelector counts under placement.
  expect(counts.coordinator).toBe(1);
  expect(counts.placement).toBe(1);
});

test("logLevel INFO is default and not counted; other levels count", () => {
  const base: TrinoConfig = { coordinator: {}, worker: {} };
  expect(sectionCounts({ ...base, logLevel: "INFO" }).general).toBe(0);
  expect(sectionCounts({ ...base, logLevel: "DEBUG" }).general).toBe(1);
});
