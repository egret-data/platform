import { sectionCounts } from "../useEngineConfigForm";
import type { TrinoConfig } from "@/api/controlPlane";

test("counts non-default field-groups per section (7 keys)", () => {
  const cfg: TrinoConfig = {
    coordinator: { replicas: 2 },
    worker: {},
    autoscaling: { mode: "hpa", hpa: { maxReplicas: 5 } },
  };
  const counts = sectionCounts(cfg);
  expect(Object.keys(counts).sort()).toEqual([
    "autoscaling",
    "coordinator",
    "image",
    "pod",
    "server",
    "worker",
    "workload",
  ]);
  // coordinator replicas → coordinator; autoscaling → autoscaling.
  expect(counts.coordinator).toBe(1);
  expect(counts.autoscaling).toBe(1);
  expect(counts.image).toBe(0);
  expect(counts.server).toBe(0);
  expect(counts.worker).toBe(0);
  expect(counts.workload).toBe(0);
  expect(counts.pod).toBe(0);
});

test("per-role sizing and placement fields both count under that role's own section", () => {
  const cfg: TrinoConfig = {
    coordinator: { nodeSelector: { disk: "ssd" }, resources: { cpuLimit: "4" } },
    worker: {},
  };
  const counts = sectionCounts(cfg);
  // Coordinator sizing (resources) and placement (nodeSelector) both count
  // under the coordinator's own section now.
  expect(counts.coordinator).toBe(2);
  expect(counts.worker).toBe(0);
});

test("logLevel INFO is default and not counted; other levels count under server", () => {
  const base: TrinoConfig = { coordinator: {}, worker: {} };
  expect(sectionCounts({ ...base, logLevel: "INFO" }).server).toBe(0);
  expect(sectionCounts({ ...base, logLevel: "DEBUG" }).server).toBe(1);
});

test("raw property maps + queryMaxMemory + eventListener count under server", () => {
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
  expect(counts.server).toBe(6);
});

it("counts image fields under the image section", () => {
  const cfg: TrinoConfig = {
    coordinator: {},
    worker: {},
    image: "ghcr.io/acme/trino:471",
    imagePullPolicy: "Always",
  };
  const counts = sectionCounts(cfg);
  expect(counts.image).toBe(2);
});
