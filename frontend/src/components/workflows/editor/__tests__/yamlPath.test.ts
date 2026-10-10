import { describe, expect, it } from "vitest";
import { rangeOfPath, typeAt } from "@/components/workflows/editor/yamlPath";

/**
 * Block and flow style, comments, a flowable with nested tasks, a task whose
 * `retry` carries its own `type:` (which is not a task type), a flow-style
 * `errors` item and a trigger. Each task has a different type, so returning
 * the wrong mapping's type cannot pass.
 */
const FLOW = [
  "id: hello", //                                                     1
  "namespace: company.team", //                                       2
  "# a comment line", //                                              3
  "tasks:", //                                                        4
  "  - id: first", //                                                 5
  "    type: io.kestra.plugin.core.flow.Sequential", //               6
  "    tasks:", //                                                    7
  "      - id: inner0", //                                            8
  "        type: io.kestra.plugin.core.log.Log", //                   9
  "        message: a", //                                           10
  "      - id: inner1   # the second child", //                      11
  "        type: io.kestra.plugin.core.debug.Return", //             12
  "        format: b", //                                            13
  "  - id: second  # trailing comment", //                           14
  "    type: io.kestra.plugin.core.log.Log", //                      15
  "    message: {text: hi}", //                                      16
  "  - id: third", //                                                17
  "    type: io.kestra.plugin.core.execution.Fail", //               18
  "    errorMessage: c", //                                          19
  "    retry: {type: constant, maxAttempt: 3, interval: PT1S}", //   20
  "errors:", //                                                      21
  "  - {id: oops, type: io.kestra.plugin.core.log.Log, message: x}", // 22
  "triggers:", //                                                    23
  "  - id: daily", //                                                24
  "    type: io.kestra.plugin.core.trigger.Schedule", //             25
  '    cron: "0 9 * * *"', //                                        26
  "",
].join("\n");
const LINES = FLOW.split("\n");

/** 1-based column of `text` on 1-based `line`, found by string search. */
const col = (line: number, text: string) => {
  const i = LINES[line - 1].indexOf(text);
  if (i < 0) throw new Error(`fixture: "${text}" is not on line ${line}`);
  return i + 1;
};
/** The range of `from` up to the end of `to` on one line. */
const span = (line: number, from: string, to: string) => ({
  startLine: line,
  startCol: col(line, from),
  endLine: line,
  endCol: col(line, to) + to.length,
});
/** The offset of the start of `text` on `line`, plus `delta`. */
const offsetOf = (line: number, text: string, delta = 0) =>
  LINES.slice(0, line - 1).reduce((n, l) => n + l.length + 1, 0) + col(line, text) - 1 + delta;

describe("rangeOfPath", () => {
  it("maps a top-level key to its key and scalar value", () => {
    expect(rangeOfPath(FLOW, "id")).toEqual(span(1, "id", "hello"));
  });

  it("maps index paths into block sequences", () => {
    expect(rangeOfPath(FLOW, "tasks[0].type")).toEqual(span(6, "type", "Sequential"));
    expect(rangeOfPath(FLOW, "tasks[0].tasks[1].id")).toEqual(span(11, "id", "inner1"));
  });

  it("maps into a flow-style mapping", () => {
    expect(rangeOfPath(FLOW, "tasks[2].retry.maxAttempt")).toEqual(span(20, "maxAttempt", "3"));
    expect(rangeOfPath(FLOW, "tasks[1].message.text")).toEqual(span(16, "text", "hi"));
  });

  it("maps a key whose value is a collection to the key alone", () => {
    expect(rangeOfPath(FLOW, "tasks[2].retry")).toEqual(span(20, "retry", "retry"));
  });

  it("maps a sequence item to its first entry, without a trailing comment", () => {
    expect(rangeOfPath(FLOW, "errors[0]")).toEqual(span(22, "{", "oops"));
    expect(rangeOfPath(FLOW, "tasks[1]")).toEqual(span(14, "id", "second"));
  });

  // The paths egret's validator emits (`spec/validate.rs`): a list of tasks,
  // inputs or triggers is read by id, and a flowable's child is named by its id
  // right after the parent's, without the list it sits in.
  it("maps the id-keyed paths the server emits", () => {
    expect(rangeOfPath(FLOW, "tasks.third.retry.maxAttempt")).toEqual(span(20, "maxAttempt", "3"));
    expect(rangeOfPath(FLOW, "tasks.first.inner1")).toEqual(span(11, "id", "inner1"));
    expect(rangeOfPath(FLOW, "tasks.first.inner1.format")).toEqual(span(13, "format", "b"));
    expect(rangeOfPath(FLOW, "errors.oops")).toEqual(span(22, "{", "oops"));
    expect(rangeOfPath(FLOW, "triggers.daily.cron")).toEqual(span(26, "cron", '"0 9 * * *"'));
  });

  it("returns null for a path into a missing node", () => {
    expect(rangeOfPath(FLOW, "tasks[7].type")).toBeNull();
    expect(rangeOfPath(FLOW, "tasks[0].nope")).toBeNull();
    expect(rangeOfPath(FLOW, "tasks.ghost")).toBeNull();
    expect(rangeOfPath(FLOW, "tasks[0].type.deeper")).toBeNull();
    expect(rangeOfPath(FLOW, "id[0]")).toBeNull();
    expect(rangeOfPath(FLOW, "")).toBeNull();
  });

  it("returns null when the document does not parse", () => {
    expect(rangeOfPath("id: [unclosed\n", "id")).toBeNull();
  });
});

describe("typeAt", () => {
  it("inside a task's property, returns that task's type", () => {
    expect(typeAt(FLOW, offsetOf(19, "errorMessage", 3))).toBe("io.kestra.plugin.core.execution.Fail");
    expect(typeAt(FLOW, offsetOf(15, "Log"))).toBe("io.kestra.plugin.core.log.Log");
  });

  it("inside a nested task, returns the inner type", () => {
    expect(typeAt(FLOW, offsetOf(13, "format", 2))).toBe("io.kestra.plugin.core.debug.Return");
    expect(typeAt(FLOW, offsetOf(10, "message"))).toBe("io.kestra.plugin.core.log.Log");
    // The flowable's own keys still belong to the flowable.
    expect(typeAt(FLOW, offsetOf(7, "tasks"))).toBe("io.kestra.plugin.core.flow.Sequential");
  });

  it("skips a mapping whose `type:` is not a task's (a retry policy has no id)", () => {
    expect(typeAt(FLOW, offsetOf(20, "maxAttempt"))).toBe("io.kestra.plugin.core.execution.Fail");
  });

  it("on the `tasks:` key itself, returns null (flow level)", () => {
    expect(typeAt(FLOW, offsetOf(4, "tasks", 2))).toBeNull();
    expect(typeAt(FLOW, offsetOf(2, "company"))).toBeNull();
  });

  it("in a trigger, returns the trigger's type", () => {
    expect(typeAt(FLOW, offsetOf(26, "9"))).toBe("io.kestra.plugin.core.trigger.Schedule");
  });

  it("in a flow-style item, returns its type", () => {
    expect(typeAt(FLOW, offsetOf(22, "message"))).toBe("io.kestra.plugin.core.log.Log");
  });

  it("returns null for a document that does not parse", () => {
    expect(typeAt("tasks: [\n  - id: a\n", 5)).toBeNull();
  });
});
