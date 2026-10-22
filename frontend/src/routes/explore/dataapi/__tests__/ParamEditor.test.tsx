import { useState } from "react";
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ParamEditor } from "@/routes/explore/dataapi/detail/ParamEditor";
import { ParamLocation, ParamType, type ParamSpec } from "@/api/generated/model";

function makeParam(overrides: Partial<ParamSpec> = {}): ParamSpec {
  return {
    name: "q",
    type: ParamType.string,
    location: ParamLocation.query,
    required: false,
    ...overrides,
  };
}

/** Drives the shadcn/Radix `Select` the way `EndpointCreate.test.tsx` does:
 *  click the trigger by its accessible (`combobox`) role, then click the
 *  option by its accessible (`option`) role. `userEvent.selectOptions` does
 *  not work here — this is not a native `<select>`. */
async function chooseType(user: ReturnType<typeof userEvent.setup>, row: string, type: string) {
  await user.click(screen.getByRole("combobox", { name: `${row} type` }));
  await user.click(await screen.findByRole("option", { name: type }));
}

describe("ParamEditor constraint fields", () => {
  // KI-026. The placeholder was the only name a constraint box had, and it
  // disappears once the box is filled: `0` and `1000000` stood under `number`
  // with nothing saying which was the minimum. Each box now has a visible label
  // tied to it by `htmlFor`/`id` (FE-FORM-2), still there when it is filled.
  it.each([
    [ParamType.number, { minimum: "0", maximum: "1000000" }, [["Minimum", "0"], ["Maximum", "1000000"]]],
    [ParamType.string, { min_length: 1, max_length: 40, pattern: "^[a-z]+$" }, [["Min length", "1"], ["Max length", "40"], ["Pattern (regex)", "^[a-z]+$"]]],
    [ParamType.date, { not_before: "2020-01-01", not_after: "2030-12-31" }, [["Not before", "2020-01-01"], ["Not after", "2030-12-31"]]],
  ] as const)("names each filled %s constraint with a visible label tied to its box", (type, values, expected) => {
    render(<ParamEditor params={[makeParam({ type, ...values })]} onChange={vi.fn()} />);

    for (const [text, value] of expected) {
      const label = screen.getByText(text, { selector: "label" });
      const input = document.getElementById(label.getAttribute("for") ?? "");
      expect(input, `${text} is tied to an input`).not.toBeNull();
      expect(input).toHaveValue(type === ParamType.string && text !== "Pattern (regex)" ? Number(value) : value);
    }
  });

  it("shows only the constraints that apply to a string parameter", () => {
    render(<ParamEditor params={[makeParam()]} onChange={vi.fn()} />);

    expect(screen.getByLabelText("Parameter 1 min length")).toBeInTheDocument();
    expect(screen.getByLabelText("Parameter 1 max length")).toBeInTheDocument();
    expect(screen.getByLabelText("Parameter 1 pattern")).toBeInTheDocument();
    expect(screen.queryByLabelText("Parameter 1 minimum")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Parameter 1 maximum")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Parameter 1 not before")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Parameter 1 not after")).not.toBeInTheDocument();
  });

  it("shows only the constraints that apply to a number parameter", () => {
    render(<ParamEditor params={[makeParam({ type: ParamType.number })]} onChange={vi.fn()} />);

    expect(screen.getByLabelText("Parameter 1 minimum")).toBeInTheDocument();
    expect(screen.getByLabelText("Parameter 1 maximum")).toBeInTheDocument();
    expect(screen.queryByLabelText("Parameter 1 pattern")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Parameter 1 min length")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Parameter 1 max length")).not.toBeInTheDocument();
  });

  it("shows only the constraints that apply to a date/timestamp parameter", () => {
    const { rerender } = render(<ParamEditor params={[makeParam({ type: ParamType.date })]} onChange={vi.fn()} />);

    expect(screen.getByLabelText("Parameter 1 not before")).toBeInTheDocument();
    expect(screen.getByLabelText("Parameter 1 not after")).toBeInTheDocument();
    expect(screen.queryByLabelText("Parameter 1 minimum")).not.toBeInTheDocument();

    rerender(<ParamEditor params={[makeParam({ type: ParamType.timestamp })]} onChange={vi.fn()} />);
    expect(screen.getByLabelText("Parameter 1 not before")).toBeInTheDocument();
    expect(screen.getByLabelText("Parameter 1 not after")).toBeInTheDocument();
  });

  it("shows no constraint inputs for boolean or enum — allowed already covers enum", () => {
    render(
      <ParamEditor
        params={[
          makeParam({ type: ParamType.boolean }),
          makeParam({ type: ParamType.enum, allowed: ["a", "b"] }),
        ]}
        onChange={vi.fn()}
      />,
    );

    expect(screen.queryByLabelText("Parameter 1 minimum")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Parameter 1 pattern")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Parameter 2 minimum")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Parameter 2 pattern")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Parameter 2 allowed values")).toBeInTheDocument();
  });

  it("switches which constraints are rendered when the author changes the type", async () => {
    const user = userEvent.setup();
    function Wrapper() {
      const [params, setParams] = useState<ParamSpec[]>([makeParam()]);
      return <ParamEditor params={params} onChange={setParams} />;
    }
    render(<Wrapper />);

    expect(screen.getByLabelText("Parameter 1 max length")).toBeInTheDocument();

    await chooseType(user, "Parameter 1", "number");

    // A `pattern` box on a number teaches a model where constraints apply
    // everywhere, and the next one an author writes silently does nothing.
    expect(screen.getByLabelText("Parameter 1 minimum")).toBeInTheDocument();
    expect(screen.queryByLabelText("Parameter 1 pattern")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Parameter 1 max length")).not.toBeInTheDocument();
  });

  it("drops a constraint that no longer applies when the type changes", async () => {
    // Sets `max_length` FIRST — `toBeUndefined()` passes trivially on a field
    // that was never set at all, so a value must be present before the type
    // change to prove the clearing logic actually ran.
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ParamEditor params={[makeParam({ max_length: 10 })]} onChange={onChange} />);

    await chooseType(user, "Parameter 1", "number");

    expect(onChange).toHaveBeenCalled();
    const last = onChange.mock.calls.at(-1)?.[0] as ParamSpec[];
    expect(last[0].type).toBe(ParamType.number);
    expect(last[0].max_length).toBeUndefined();
  });

  it("clears every inapplicable constraint at once, not just the one under test", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <ParamEditor
        params={[makeParam({ min_length: 1, max_length: 10, pattern: "^a" })]}
        onChange={onChange}
      />,
    );

    await chooseType(user, "Parameter 1", "number");

    const last = onChange.mock.calls.at(-1)?.[0] as ParamSpec[];
    expect(last[0].min_length).toBeUndefined();
    expect(last[0].max_length).toBeUndefined();
    expect(last[0].pattern).toBeUndefined();
  });

  it("drops `allowed` when an enum becomes something else, and keeps it otherwise", async () => {
    // The editor shows `allowed` for `enum` alone, so a list left behind by a
    // type change is invisible — and since the backend refuses `allowed` on any
    // other type (`ConstraintNotForType`), invisible means unfixable from this
    // screen. Set FIRST, so `toBeUndefined()` cannot pass on a field that was
    // never there.
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <ParamEditor
        params={[makeParam({ type: ParamType.enum, allowed: ["a", "b"] })]}
        onChange={onChange}
      />,
    );

    await chooseType(user, "Parameter 1", "string");

    const last = onChange.mock.calls.at(-1)?.[0] as ParamSpec[];
    expect(last[0].type).toBe(ParamType.string);
    expect(last[0].allowed).toBeUndefined();
  });

  it("keeps `allowed` when the type is set to enum again", async () => {
    // The other direction, without which the fix above could have been
    // "always clear `allowed`" and every enum would lose its list on any
    // re-selection of its own type.
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <ParamEditor
        params={[makeParam({ type: ParamType.string, allowed: ["a", "b"] })]}
        onChange={onChange}
      />,
    );

    await chooseType(user, "Parameter 1", "enum");

    const last = onChange.mock.calls.at(-1)?.[0] as ParamSpec[];
    expect(last[0].type).toBe(ParamType.enum);
    expect(last[0].allowed).toEqual(["a", "b"]);
  });

  it("preserves not_before/not_after across a date <-> timestamp switch", async () => {
    // Both types share the same two fields — only fields the NEW type does
    // not use should be cleared.
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <ParamEditor
        params={[makeParam({ type: ParamType.date, not_before: "2026-01-01" })]}
        onChange={onChange}
      />,
    );

    await chooseType(user, "Parameter 1", "timestamp");

    const last = onChange.mock.calls.at(-1)?.[0] as ParamSpec[];
    expect(last[0].not_before).toBe("2026-01-01");
  });

  it("writes minimum/maximum as plain strings, never coerced through a number", async () => {
    // A decimal round-tripped through the browser's float would stop
    // comparing equal to itself on the engine — the wire type is `string`
    // and the input must not silently promote it to a JS number. Wrapped in
    // a stateful component: the field is fully controlled by `params`, so a
    // bare `vi.fn()` spy would never accumulate keystrokes past the first
    // (each one reverts to the unchanged prop before the next fires).
    const user = userEvent.setup();
    let lastParams: ParamSpec[] = [];
    function Wrapper() {
      const [params, setParams] = useState<ParamSpec[]>([makeParam({ type: ParamType.number })]);
      lastParams = params;
      return (
        <ParamEditor
          params={params}
          onChange={(next) => {
            lastParams = next;
            setParams(next);
          }}
        />
      );
    }
    render(<Wrapper />);

    await user.type(screen.getByLabelText("Parameter 1 minimum"), "1.50");

    expect(lastParams[0].minimum).toBe("1.50");
    expect(typeof lastParams[0].minimum).toBe("string");
  });
});

/**
 * The empty state is the only place the editor tells an author how to reference
 * a parameter from SQL, and it used to teach `:name` — a syntax the renderer
 * has never had. `crates/egret-dataapi/src/render.rs` is minijinja with a bind
 * formatter and contains no `:name` handling at all, so an author who followed
 * the hint wrote a template referencing nothing and was refused at publish for
 * an unreferenced parameter.
 */
describe("ParamEditor empty state", () => {
  it("tells the author the placeholder syntax the renderer actually accepts", () => {
    render(<ParamEditor params={[]} onChange={vi.fn()} />);

    // The whole sentence, as rendered — the `<code>` is a child element, so
    // reading the cell is what proves the hint reaches the screen.
    const hint = screen.getByText(/no parameters yet/i);
    expect(hint).toHaveTextContent("{{ name }}");
    // Not merely "the new string is present": the wrong one must be gone.
    expect(hint.textContent).not.toContain(":name");
  });

  it("still says where the value comes from", () => {
    // The clause either side of the placeholder stayed true and must survive
    // the edit — `location` defaults to `query` and the empty state is the
    // only statement of that on this screen.
    render(<ParamEditor params={[]} onChange={vi.fn()} />);

    expect(screen.getByText(/no parameters yet/i)).toHaveTextContent(
      "read from the query string",
    );
  });
});
