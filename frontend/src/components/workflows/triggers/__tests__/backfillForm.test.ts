import { describe, expect, it } from "vitest";
import { inputsFromSource, formFromInputs } from "@/components/workflows/run/inputModel";
import {
  bodyFromForm,
  labelRowError,
  offeredInputs,
  placeError,
  scheduleInputs,
  submitReasons,
  type BackfillForm,
} from "@/components/workflows/triggers/backfillForm";

const NOW = new Date("2026-10-15T12:00:00Z");

function form(over: Partial<BackfillForm> = {}): BackfillForm {
  return { start: "2020-01-01T00:00:00", end: "", labels: [], inputs: formFromInputs([]), ...over };
}

describe("bodyFromForm", () => {
  it("sends the local values as UTC instants, leaves an empty end out, and drops empty label rows", () => {
    const body = bodyFromForm(
      form({
        labels: [
          { key: "team", value: "ops" },
          { key: "", value: "" },
        ],
      }),
    );
    expect(body).toEqual({
      start: new Date("2020-01-01T00:00:00").toISOString(),
      inputs: {},
      labels: [{ key: "team", value: "ops" }],
    });
    expect(body).not.toHaveProperty("end");
  });
});

describe("labelRowError", () => {
  it("reserves the system. prefix and wants both halves of a row", () => {
    expect(labelRowError({ key: "system.x", value: "1" })).toBe("Keys starting with system. are reserved");
    expect(labelRowError({ key: "team", value: "" })).toBe("Fill both the key and the value");
    expect(labelRowError({ key: "", value: "ops" })).toBe("Fill both the key and the value");
    expect(labelRowError({ key: "", value: "" })).toBeNull();
  });
});

describe("submitReasons", () => {
  it("names the first thing that keeps the submit off", () => {
    expect(submitReasons(form({ start: "" }), NOW)).toContain("Pick a start");
    expect(submitReasons(form({ start: "2020-01-02T00:00:00", end: "2020-01-01T00:00:00" }), NOW)).toContain(
      "The start must be before the end",
    );
    expect(submitReasons(form({ end: "2099-01-01T00:00:00" }), NOW)).toContain(
      "The end is in the future; leave it empty to end now",
    );
    expect(submitReasons(form({ labels: [{ key: "team", value: "" }] }), NOW)).toContain("Fix the labels");
    expect(submitReasons(form(), NOW)).toEqual([]);
  });
});

describe("placeError", () => {
  it("puts each refusal beside the field it names", () => {
    expect(placeError("backfill_start_too_early", "too early", [])).toEqual({ field: "start", message: "too early" });
    expect(placeError("backfill_window_invalid", "bad", [])).toEqual({ field: "end", message: "bad" });
    expect(placeError("backfill_labels_invalid", "bad", [])).toEqual({ field: "labels", message: "bad" });
    expect(placeError("backfill_no_ticks", "none", [])).toEqual({ field: "form", message: "none" });
  });

  it("names the input a refused input message names, else keeps it on the form", () => {
    const parsed = inputsFromSource("inputs:\n  - id: req\n    type: STRING\n");
    const declared = "inputs" in parsed ? parsed.inputs : [];
    expect(placeError("backfill_inputs_invalid", "req is required", declared)).toEqual({
      field: "input",
      input: "req",
      message: "req is required",
    });
    expect(placeError("backfill_inputs_invalid", "something else", declared)).toEqual({ field: "form", message: "something else" });
  });
});

describe("scheduleInputs and offeredInputs", () => {
  const source = [
    "id: hello",
    "inputs:",
    "  - id: region",
    "    type: STRING",
    "  - id: upload",
    "    type: FILE",
    "    required: false",
    "triggers:",
    "  - id: daily",
    "    type: io.kestra.plugin.core.trigger.Schedule",
    "    inputs:",
    "      region: eu",
    '      when: "{{ now() }}"',
    "tasks: []",
  ].join("\n");

  it("prefills from the plain schedule values and leaves a template out", () => {
    expect(scheduleInputs(source, "daily")).toEqual({ region: "eu" });
  });

  it("offers every input but FILE", () => {
    const declared = inputsFromSource(source);
    expect("inputs" in declared && offeredInputs(declared.inputs).map((i) => i.id)).toEqual(["region"]);
  });
});
