import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { TriggerLabel, triggerKind } from "@/components/workflows/executions/trigger";

type Row = Parameters<typeof triggerKind>[0];
const base: Row = { execution_trigger: null as unknown as Row["execution_trigger"], parent_execution_id: null, loop_index: null, replayed_from_execution_id: null };
const by = (type: string): Row => ({ ...base, execution_trigger: { id: "t", type, variables: {} } });

describe("triggerKind", () => {
  it("names each trigger type the engine writes", () => {
    expect(triggerKind(by("io.kestra.plugin.core.trigger.Schedule"))).toEqual({ kind: "schedule", label: "Schedule" });
    expect(triggerKind(by("io.kestra.plugin.core.trigger.Webhook"))).toEqual({ kind: "webhook", label: "Webhook" });
    expect(triggerKind(by("io.kestra.plugin.core.trigger.Flow"))).toEqual({ kind: "flow", label: "Flow trigger" });
    expect(triggerKind(by("io.kestra.plugin.core.flow.Subflow"))).toEqual({ kind: "subflow", label: "Subflow" });
  });

  it("is API for a start with no trigger record", () => {
    expect(triggerKind(base)).toEqual({ kind: "api", label: "API" });
    // W7a's fixtures carry `{}`, as an older row may: no type is no trigger.
    expect(triggerKind({ ...base, execution_trigger: {} })).toEqual({ kind: "api", label: "API" });
  });

  it("tells a loop iteration, a retry and a replay from an API start, though none carries a record", () => {
    expect(triggerKind({ ...base, parent_execution_id: "p", loop_index: 0 })).toEqual({ kind: "loop", label: "Loop iteration" });
    expect(triggerKind({ ...base, parent_execution_id: "p" })).toEqual({ kind: "retry", label: "Retry" });
    expect(triggerKind({ ...base, replayed_from_execution_id: "r" })).toEqual({ kind: "replay", label: "Replay" });
  });

  it("shows a type it does not know by its last segment, rather than guessing", () => {
    expect(triggerKind(by("io.kestra.plugin.kafka.Trigger"))).toEqual({ kind: "other", label: "Trigger" });
  });
});

describe("TriggerLabel", () => {
  it("shows the word beside a hidden glyph, so the trigger is never an icon alone", () => {
    const { container } = render(<TriggerLabel row={by("io.kestra.plugin.core.trigger.Schedule")} />);
    expect(screen.getByText("Schedule")).toBeInTheDocument();
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg).toHaveAttribute("aria-hidden", "true");
  });

  it("titles an unknown type with its full name", () => {
    render(<TriggerLabel row={by("io.kestra.plugin.kafka.Trigger")} />);
    expect(screen.getByText("Trigger").closest("[title]")).toHaveAttribute("title", "io.kestra.plugin.kafka.Trigger");
  });
});
