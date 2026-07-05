import { sectionCounts } from "../useEngineConfigForm";
import type { TrinoConfig } from "@/api/controlPlane";

test("counts non-default field-groups per section (5 keys)", () => {
  const cfg: TrinoConfig = {
    coordinator: { replicas: 2 },
    worker: {},
    autoscaling: { mode: "hpa", hpa: { maxReplicas: 5 } },
  };
  const counts = sectionCounts(cfg);
  expect(Object.keys(counts).sort()).toEqual([
    "properties",
    "resources",
    "scaling",
    "scheduling",
    "workload",
  ]);
  // coordinator replicas → resources; autoscaling → scaling.
  expect(counts.resources).toBe(1);
  expect(counts.scaling).toBe(1);
  expect(counts.properties).toBe(0);
  expect(counts.workload).toBe(0);
  expect(counts.scheduling).toBe(0);
});

test("per-role sizing counts under resources, scheduling fields under scheduling", () => {
  const cfg: TrinoConfig = {
    coordinator: { nodeSelector: { disk: "ssd" }, resources: { cpuLimit: "4" } },
    worker: {},
  };
  const counts = sectionCounts(cfg);
  // resources counts the coordinator sizing; nodeSelector counts under scheduling.
  expect(counts.resources).toBe(1);
  expect(counts.scheduling).toBe(1);
});

test("logLevel INFO is default and not counted; other levels count under properties", () => {
  const base: TrinoConfig = { coordinator: {}, worker: {} };
  expect(sectionCounts({ ...base, logLevel: "INFO" }).properties).toBe(0);
  expect(sectionCounts({ ...base, logLevel: "DEBUG" }).properties).toBe(1);
});

test("raw property maps + queryMaxMemory + eventListener count under properties", () => {
  const cfg: TrinoConfig = {
    coordinator: {},
    worker: {},
    queryMaxMemory: "8GB",
    configProperties: { "spill-enabled": "true", "query.max-run-time": "1h" },
    additionalNodeProperties: { "node.environment": "prod" },
    additionalLogProperties: { "io.trino": "DEBUG" },
    eventListener: { "event-listener.name": "http" },
  };
  const counts = sectionCounts(cfg);
  // 2 config + 1 node + 1 log map keys + queryMaxMemory + eventListener = 6.
  expect(counts.properties).toBe(6);
});
