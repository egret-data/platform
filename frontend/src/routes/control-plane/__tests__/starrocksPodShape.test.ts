import { describe, expect, it } from "vitest";

import type { Kv } from "../EngineResourceGroups";
import type { TolerationForm } from "../useEngineConfigForm";
import {
  containersJsonToForms,
  envJsonToKv,
  formsToContainersJson,
  kvToEnvJson,
  kvToRecord,
  recordToKv,
  rowsToTolerationsJson,
  tolerationsJsonToRows,
} from "../starrocksPodShape";

describe("env <-> Kv", () => {
  it("round-trips [{name,value}] <-> Kv[]", () => {
    const json = [{ name: "TZ", value: "UTC" }];
    const rows = envJsonToKv(json);
    expect(rows).toEqual<Kv[]>([{ key: "TZ", value: "UTC" }]);
    expect(kvToEnvJson(rows)).toEqual(json);
  });

  it("round-trips multiple entries preserving order", () => {
    const json = [
      { name: "A", value: "1" },
      { name: "B", value: "2" },
    ];
    expect(kvToEnvJson(envJsonToKv(json))).toEqual(json);
  });

  it("empty [] round-trips cleanly", () => {
    expect(envJsonToKv([])).toEqual([]);
    expect(kvToEnvJson([])).toEqual([]);
  });

  it("drops blank-key rows on serialize (matches kvToRecord)", () => {
    const rows: Kv[] = [
      { key: "", value: "orphan" },
      { key: "OK", value: "v" },
    ];
    expect(kvToEnvJson(rows)).toEqual([{ name: "OK", value: "v" }]);
  });

  it("does not throw on malformed input (string / null)", () => {
    expect(envJsonToKv("nope")).toEqual([]);
    expect(envJsonToKv(null)).toEqual([]);
    expect(envJsonToKv(undefined)).toEqual([]);
    expect(envJsonToKv({ not: "array" })).toEqual([]);
  });
});

describe("record <-> Kv", () => {
  it("round-trips {a,b} <-> Kv[] in stable insertion order", () => {
    const rec = { a: "1", b: "2" };
    const rows = recordToKv(rec);
    expect(rows).toEqual<Kv[]>([
      { key: "a", value: "1" },
      { key: "b", value: "2" },
    ]);
    expect(kvToRecord(rows)).toEqual(rec);
  });

  it("empty {} round-trips cleanly", () => {
    expect(recordToKv({})).toEqual([]);
    expect(kvToRecord([])).toEqual({});
  });

  it("does not throw on malformed input", () => {
    expect(recordToKv(null)).toEqual([]);
    expect(recordToKv("x")).toEqual([]);
    expect(recordToKv(42)).toEqual([]);
  });
});

describe("tolerations <-> TolerationForm", () => {
  it("round-trips a full toleration with numeric tolerationSeconds", () => {
    const json = [
      {
        key: "dedicated",
        operator: "Equal",
        value: "analytics",
        effect: "NoSchedule",
        tolerationSeconds: 30,
      },
    ];
    const rows = tolerationsJsonToRows(json);
    expect(rows).toEqual<TolerationForm[]>([
      {
        key: "dedicated",
        operator: "Equal",
        value: "analytics",
        effect: "NoSchedule",
        tolerationSeconds: "30",
      },
    ]);
    expect(rowsToTolerationsJson(rows)).toEqual(json);
  });

  it("maps absent tolerationSeconds to '' and omits it on serialize", () => {
    const json = [{ key: "k", operator: "Exists", effect: "NoExecute" }];
    const rows = tolerationsJsonToRows(json);
    expect(rows[0].tolerationSeconds).toBe("");
    expect(rows[0].value).toBe("");
    // value was absent -> omitted; tolerationSeconds absent -> omitted.
    expect(rowsToTolerationsJson(rows)).toEqual([
      { key: "k", operator: "Exists", effect: "NoExecute" },
    ]);
  });

  it("does not throw on malformed input (null / string)", () => {
    expect(tolerationsJsonToRows(null)).toEqual([]);
    expect(tolerationsJsonToRows("bad")).toEqual([]);
    expect(rowsToTolerationsJson([])).toEqual([]);
  });
});

describe("containers <-> ContainerForm", () => {
  it("round-trips name/image and k8s env array <-> record", () => {
    const json = [
      {
        name: "wait-for-dep",
        image: "busybox:1.36",
        command: ["sh", "-c"],
        args: ["sleep 1"],
        env: [{ name: "TZ", value: "UTC" }],
        volumeMounts: [{ name: "data", mountPath: "/mnt/data" }],
      },
    ];
    const forms = containersJsonToForms(json);
    expect(forms[0].name).toBe("wait-for-dep");
    expect(forms[0].image).toBe("busybox:1.36");
    expect(forms[0].env).toEqual({ TZ: "UTC" });
    expect(forms[0].command).toEqual(["sh", "-c"]);
    expect(forms[0].rawOverride).toBeUndefined();
    expect(formsToContainersJson(forms)).toEqual(json);
  });

  it("preserves an unknown container key through the raw override", () => {
    const json = [
      {
        name: "c",
        image: "img",
        resources: { limits: { cpu: "2" } },
        imagePullPolicy: "IfNotPresent",
      },
    ];
    const forms = containersJsonToForms(json);
    expect(forms[0].rawOverride).toEqual({
      resources: { limits: { cpu: "2" } },
      imagePullPolicy: "IfNotPresent",
    });
    // full round-trip: unknown keys re-merged, nothing dropped.
    expect(formsToContainersJson(forms)).toEqual(json);
  });

  it("round-trips an empty container list", () => {
    expect(containersJsonToForms([])).toEqual([]);
    expect(formsToContainersJson([])).toEqual([]);
  });

  it("does not throw on malformed input (null / object)", () => {
    expect(containersJsonToForms(null)).toEqual([]);
    expect(containersJsonToForms({ not: "array" })).toEqual([]);
  });

  it("preserves a non-object list entry verbatim", () => {
    const json = ["weird-entry"];
    const forms = containersJsonToForms(json);
    expect(formsToContainersJson(forms)).toEqual(json);
  });
});
