import { sectionCounts } from "../useEngineConfigForm";
import type { TrinoConfig } from "@/api/controlPlane";

test("counts non-default field-groups per section (8 keys)", () => {
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
    "security",
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
  expect(counts.security).toBe(0);
});

test("security counts password users, JWT acceptance, and ingress exposure", () => {
  const base: TrinoConfig = { coordinator: {}, worker: {} };
  expect(sectionCounts(base).security).toBe(0);
  // Password users present → 1.
  expect(
    sectionCounts({
      ...base,
      auth: { passwordUsers: [{ username: "u", secretRef: "s" }] },
    }).security,
  ).toBe(1);
  // Password users + JWT + ingress → 3.
  expect(
    sectionCounts({
      ...base,
      auth: {
        passwordUsers: [{ username: "u", secretRef: "s" }],
        jwt: { jwksUri: "http://kc/jwks" },
      },
      ingress: { enabled: true, host: "trino.example.com" },
    }).security,
  ).toBe(3);
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

test("log level and query.max-memory count as ordinary map entries under server", () => {
  const base: TrinoConfig = { coordinator: {}, worker: {} };
  // Server card removed: the log level is an io.trino entry in log.properties,
  // and query.max-memory a config.properties entry — each counts once via its map.
  expect(sectionCounts(base).server).toBe(0);
  expect(
    sectionCounts({ ...base, additionalLogProperties: { "io.trino": "DEBUG" } })
      .server,
  ).toBe(1);
  expect(
    sectionCounts({ ...base, configProperties: { "query.max-memory": "8GB" } })
      .server,
  ).toBe(1);
});

test("raw property maps + eventListener count under server", () => {
  const cfg: TrinoConfig = {
    coordinator: {},
    worker: {},
    configProperties: { "query.max-memory": "8GB", "query.max-run-time": "1h" },
    additionalNodeProperties: { "node.environment": "prod" },
    additionalLogProperties: { "io.trino": "DEBUG" },
    eventListener: { "event-listener.name": "http" },
  };
  const counts = sectionCounts(cfg);
  // 2 config + 1 node + 1 log map keys + eventListener = 5.
  expect(counts.server).toBe(5);
});

test("reliability retry-attempt caps count once under server (not doubled)", () => {
  const cfg: TrinoConfig = {
    coordinator: {},
    worker: {},
    // query-retry-attempts is a reliability known-prop stored in configProperties;
    // it must be counted once via the map, not a second time via a section scan.
    configProperties: { "query-retry-attempts": "4", "spill-enabled": "true" },
  };
  expect(sectionCounts(cfg).server).toBe(2);
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
