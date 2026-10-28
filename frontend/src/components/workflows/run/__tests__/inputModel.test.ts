import { describe, expect, it } from "vitest";
import {
  containerFor,
  formFromInputs,
  inputsFromSource,
  requestFromForm,
  serverErrorInput,
  serverErrorsByInput,
  unsupportedInputs,
  validateForm,
  validationRequestFromForm,
  validationView,
  type FormState,
  type RunInput,
} from "@/components/workflows/run/inputModel";

/** The `inputs:` of a flow, parsed the way the run form reads the saved draft. */
function inputs(yaml: string): RunInput[] {
  const parsed = inputsFromSource(`id: f\nnamespace: n\ninputs:\n${yaml}\ntasks: []\n`);
  if ("error" in parsed) throw new Error(parsed.error);
  return parsed.inputs;
}

function one(yaml: string): { input: RunInput; form: FormState } {
  const list = inputs(yaml);
  expect(list).toHaveLength(1);
  return { input: list[0], form: formFromInputs(list) };
}

function set(form: FormState, id: string, value: FormState["values"][string]): FormState {
  return { ...form, values: { ...form.values, [id]: value } };
}

/** The JSON body's `inputs`, failing when the request is multipart. */
function jsonInputs(form: FormState): Record<string, unknown> {
  const req = requestFromForm(form);
  if (!("json" in req)) throw new Error("expected a JSON request");
  return req.json.inputs;
}

describe("inputsFromSource", () => {
  it("reads id, type, displayName, description, required (true unless false), values and itemType", () => {
    const list = inputs(
      [
        "  - id: region",
        "    type: SELECT",
        "    displayName: Region",
        "    description: Where to run",
        "    values: [eu, us, 3]",
        "  - id: tags",
        "    type: ARRAY",
        "    itemType: INT",
        "    required: false",
      ].join("\n"),
    );
    expect(list[0]).toMatchObject({
      id: "region",
      type: "SELECT",
      displayName: "Region",
      description: "Where to run",
      required: true,
      values: ["eu", "us", "3"],
    });
    expect(list[1]).toMatchObject({ id: "tags", type: "ARRAY", itemType: "INT", required: false });
  });

  it("reads a MULTISELECT's deprecated `options` when it has no `values`", () => {
    expect(inputs("  - id: m\n    type: MULTISELECT\n    options: [a, b]")[0].values).toEqual(["a", "b"]);
  });

  it("flags dependsOn", () => {
    const [a, b] = inputs("  - id: a\n    type: STRING\n  - id: b\n    type: STRING\n    dependsOn:\n      inputs: [a]");
    expect(a.dependsOn).toBe(false);
    expect(b.dependsOn).toBe(true);
  });

  it("answers no inputs for a flow without an inputs block", () => {
    expect(inputsFromSource("id: f\nnamespace: n\ntasks: []\n")).toEqual({ inputs: [] });
  });

  it("reports a source that does not parse rather than answering no inputs", () => {
    expect(inputsFromSource("id: [unclosed\n")).toHaveProperty("error");
  });
});

describe("formFromInputs and requestFromForm, per type (spec §6.1)", () => {
  it("STRING, EMAIL, URI, SECRET and DURATION: the default fills in and is sent as text", () => {
    for (const type of ["STRING", "EMAIL", "URI", "SECRET", "DURATION"]) {
      const { form } = one(`  - id: x\n    type: ${type}\n    defaults: PT1H`);
      expect(form.values.x).toBe("PT1H");
      expect(jsonInputs(form)).toEqual({ x: "PT1H" });
    }
  });

  it("a STRING default that YAML reads as a number or a boolean stays text", () => {
    const { form } = one("  - id: x\n    type: STRING\n    defaults: true");
    expect(jsonInputs(form)).toEqual({ x: "true" });
  });

  it("INT and FLOAT: the default fills in, and the value is sent as a JSON number", () => {
    const { form } = one("  - id: n\n    type: INT\n    defaults: 42");
    expect(form.values.n).toBe("42");
    expect(jsonInputs(set(form, "n", "7"))).toEqual({ n: 7 });
    const f = one("  - id: f\n    type: FLOAT\n    defaults: 1.5").form;
    expect(jsonInputs(f)).toEqual({ f: 1.5 });
  });

  it("BOOL: the default fills in, an absent default is false, and the value is a JSON boolean", () => {
    expect(jsonInputs(one("  - id: b\n    type: BOOL\n    defaults: true").form)).toEqual({ b: true });
    const { form } = one("  - id: b\n    type: BOOL");
    expect(form.values.b).toBe(false);
    expect(jsonInputs(form)).toEqual({ b: false });
  });

  it("DATETIME: the default shows as local wall time and is sent as an ISO instant", () => {
    const { form } = one("  - id: at\n    type: DATETIME\n    defaults: 2026-03-04T05:06:07Z");
    const shown = form.values.at as string;
    // What a `datetime-local` control holds: no zone, no milliseconds.
    expect(shown).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}$/);
    expect(new Date(shown).toISOString()).toBe("2026-03-04T05:06:07.000Z");
    expect(jsonInputs(form)).toEqual({ at: "2026-03-04T05:06:07.000Z" });
  });

  it("DATE is sent as YYYY-MM-DD, and TIME as HH:MM:SS even when the control gives HH:MM", () => {
    expect(jsonInputs(one("  - id: d\n    type: DATE\n    defaults: 2026-03-04").form)).toEqual({ d: "2026-03-04" });
    const t = one("  - id: t\n    type: TIME").form;
    expect(jsonInputs(set(t, "t", "09:30"))).toEqual({ t: "09:30:00" });
    expect(jsonInputs(set(t, "t", "09:30:15"))).toEqual({ t: "09:30:15" });
  });

  it("SELECT is sent as its value; MULTISELECT as an array of the chosen values", () => {
    const s = one("  - id: s\n    type: SELECT\n    values: [a, b]\n    defaults: b").form;
    expect(s.values.s).toBe("b");
    expect(jsonInputs(s)).toEqual({ s: "b" });
    const m = one("  - id: m\n    type: MULTISELECT\n    values: [a, b, c]\n    defaults: [c, a]").form;
    expect(m.values.m).toEqual(["c", "a"]);
    expect(jsonInputs(m)).toEqual({ m: ["c", "a"] });
  });

  it("ARRAY is sent as an array typed by itemType, blank rows dropped", () => {
    const ints = one("  - id: a\n    type: ARRAY\n    itemType: INT\n    defaults: [1, 2]").form;
    expect(ints.values.a).toEqual(["1", "2"]);
    expect(jsonInputs(set(ints, "a", ["3", "", "10"]))).toEqual({ a: [3, 10] });
    const strs = one("  - id: a\n    type: ARRAY\n    itemType: STRING").form;
    expect(jsonInputs(set(strs, "a", ["1", "x"]))).toEqual({ a: ["1", "x"] });
  });

  it("JSON: a structured default fills in as text, and the value is sent as its text for the backend to parse once", () => {
    const { form } = one("  - id: j\n    type: JSON\n    defaults:\n      k: [1, 2]");
    expect(JSON.parse(form.values.j as string)).toEqual({ k: [1, 2] });
    expect(jsonInputs(set(form, "j", '{"a": {"b": null}}'))).toEqual({ j: '{"a": {"b": null}}' });
  });

  it("JSON: a JSON string is sent as its text, so the backend's one parse leaves a string, not a number", () => {
    const { form } = one("  - id: j\n    type: JSON");
    // Typed `"123"` (with the quotes). Sent parsed it would be the string 123, which the
    // backend parses again into the number 123 (`with_declared_defaults` parses a JSON input's string).
    const sent = jsonInputs(set(form, "j", '"123"')).j;
    expect(sent).toBe('"123"');
    expect(JSON.parse(sent as string)).toBe("123");
  });

  it("YAML: sent as the text typed, unparsed (the backend keeps a YAML input as a string)", () => {
    const { form } = one("  - id: y\n    type: YAML\n    defaults:\n      k: v");
    expect(form.values.y).toBe("k: v\n");
    expect(jsonInputs(set(form, "y", "a: 1\nb: [x]\n"))).toEqual({ y: "a: 1\nb: [x]\n" });
  });

  it("FILE: a chosen file makes the request multipart, the file under its input id and every other value a text part", () => {
    const list = inputs(
      [
        "  - id: data",
        "    type: FILE",
        "  - id: n",
        "    type: INT",
        "  - id: s",
        "    type: STRING",
        "  - id: j",
        "    type: JSON",
      ].join("\n"),
    );
    const file = new File(["a,b\n"], "rows.csv", { type: "text/csv" });
    const form = { ...formFromInputs(list), values: { data: file, n: "5", s: "plain", j: '{"x": 1}' } };
    const req = requestFromForm(form);
    if (!("multipart" in req)) throw new Error("expected multipart");
    const got = req.multipart.get("data");
    expect(got).toBeInstanceOf(File);
    expect((got as File).name).toBe("rows.csv");
    // Text parts: the backend parses a structured type's text as JSON, and keeps a string as it is.
    expect(req.multipart.get("n")).toBe("5");
    expect(req.multipart.get("s")).toBe("plain");
    expect(JSON.parse(req.multipart.get("j") as string)).toEqual({ x: 1 });
    expect([...req.multipart.keys()].sort()).toEqual(["data", "j", "n", "s"]);
  });

  it("FILE: with no file chosen the request stays JSON and the input is left out", () => {
    const { form } = one("  - id: data\n    type: FILE\n    required: false");
    expect(jsonInputs(form)).toEqual({});
  });

  it("leaves out an empty optional value, so the flow's own default applies", () => {
    const list = inputs("  - id: s\n    type: STRING\n    required: false\n  - id: m\n    type: MULTISELECT\n    values: [a]\n    required: false");
    expect(jsonInputs(formFromInputs(list))).toEqual({});
  });

  it("an expression default is not filled in", () => {
    const { input, form } = one('  - id: at\n    type: DATETIME\n    defaults: "{{ now() }}"');
    expect(form.values.at).toBe("");
    expect(input.expressionDefault).toBe("{{ now() }}");
  });
});

describe("validateForm", () => {
  it("blocks a required, empty value with no default, and lets an optional one through", () => {
    const list = inputs("  - id: need\n    type: STRING\n  - id: opt\n    type: STRING\n    required: false");
    expect(validateForm(formFromInputs(list))).toEqual({ need: "Enter a value for this input." });
  });

  it("blocks each empty kind: a select, a multi-select, an array, a file", () => {
    const list = inputs(
      [
        "  - id: s",
        "    type: SELECT",
        "    values: [a]",
        "  - id: m",
        "    type: MULTISELECT",
        "    values: [a]",
        "  - id: a",
        "    type: ARRAY",
        "    itemType: STRING",
        "  - id: f",
        "    type: FILE",
      ].join("\n"),
    );
    const form = formFromInputs(list);
    const errors = validateForm(set(form, "a", ["", "  "]));
    expect(Object.keys(errors).sort()).toEqual(["a", "f", "m", "s"]);
    expect(errors.f).toBe("Choose a file.");
  });

  it("does not block a required input the flow has a default for", () => {
    const { form } = one("  - id: s\n    type: STRING\n    defaults: x");
    expect(validateForm(set(form, "s", ""))).toEqual({});
  });

  it("checks EMAIL, URI and DURATION before asking the server", () => {
    const list = inputs("  - id: e\n    type: EMAIL\n  - id: u\n    type: URI\n  - id: d\n    type: DURATION");
    const check = (values: Record<string, string>) => validateForm({ ...formFromInputs(list), values });
    expect(check({ e: "abc", u: "example.com", d: "1h" })).toEqual({
      e: "Enter an email address, such as name@example.com.",
      u: "Enter a URI with its scheme, such as https://example.com.",
      d: "Enter an ISO-8601 duration, such as PT1H or P1DT12H.",
    });
    expect(check({ e: "ada@example.com", u: "https://example.com/x?y=1", d: "PT1H" })).toEqual({});
    for (const ok of ["P1D", "P1DT12H", "PT30M", "PT0.5S", "pt1h", "-PT6H", "PT-6H"]) {
      expect(check({ e: "a@b.co", u: "s3://b/k", d: ok }), ok).toEqual({});
    }
    // Java's `Duration` has no years, months or weeks, and needs a component after P and after T.
    for (const bad of ["P", "PT", "P1Y", "P1M", "P1W", "P1DT", "1H"]) {
      expect(check({ e: "a@b.co", u: "s3://b/k", d: bad }), bad).toHaveProperty("d");
    }
  });

  it("refuses a malformed number, JSON and array item, naming what is wrong", () => {
    const list = inputs(
      "  - id: n\n    type: INT\n  - id: j\n    type: JSON\n  - id: a\n    type: ARRAY\n    itemType: FLOAT",
    );
    const errors = validateForm({ ...formFromInputs(list), values: { n: "1.5", j: "{nope", a: ["1", "x"] } });
    expect(errors.n).toBe("Enter a whole number.");
    expect(errors.j).toMatch(/^This is not valid JSON/);
    expect(errors.a).toBe("Item 2: enter a number.");
  });
});

describe("unsupportedInputs and containerFor", () => {
  it("reports FORM, REUSABLE_INPUTS, ION and an unknown type, with their ids and types", () => {
    const list = inputs(
      [
        "  - id: ok",
        "    type: STRING",
        "  - id: grp",
        "    type: FORM",
        "  - id: re",
        "    type: REUSABLE_INPUTS",
        "  - id: i",
        "    type: ION",
        "  - id: weird",
        "    type: BLOB",
      ].join("\n"),
    );
    expect(unsupportedInputs(list)).toEqual([
      { id: "grp", type: "FORM" },
      { id: "re", type: "REUSABLE_INPUTS" },
      { id: "i", type: "ION" },
      { id: "weird", type: "BLOB" },
    ]);
  });

  it("chooses confirm for none, a dialog for at most three scalars, a drawer otherwise", () => {
    const scalar = (n: number) =>
      inputs(Array.from({ length: n }, (_, i) => `  - id: s${i}\n    type: ${["STRING", "INT", "BOOL"][i % 3]}`).join("\n"));
    expect(containerFor([])).toBe("confirm");
    expect(containerFor(scalar(1))).toBe("dialog");
    expect(containerFor(scalar(3))).toBe("dialog");
    expect(containerFor(scalar(4))).toBe("drawer");
    expect(containerFor(inputs("  - id: j\n    type: JSON"))).toBe("drawer");
    expect(containerFor(inputs("  - id: f\n    type: FILE"))).toBe("drawer");
    expect(containerFor(inputs("  - id: m\n    type: MULTISELECT\n    values: [a]"))).toBe("drawer");
  });
});

describe("validateForm: bounds and listed values", () => {
  const check = (yaml: string, value: FormState["values"][string]) => {
    const { input, form } = one(yaml);
    return validateForm(set(form, input.id, value))[input.id];
  };

  it("reads min, max, after, before, validator and allowCustomValue from the YAML as text", () => {
    const [n, d] = inputs(
      [
        "  - id: n",
        "    type: INT",
        "    min: 10",
        "    max: 20",
        "  - id: d",
        "    type: DATE",
        "    after: 2023-01-01",
        "    before: 2023-12-31",
        '    validator: "\\\\d+"',
        "    allowCustomValue: true",
      ].join("\n"),
    );
    expect(n).toMatchObject({ min: "10", max: "20", allowCustomValue: false });
    expect(d).toMatchObject({ after: "2023-01-01", before: "2023-12-31", validator: "\\d+", allowCustomValue: true });
  });

  it("INT and FLOAT: min and max, both inclusive", () => {
    const int = "  - id: n\n    type: INT\n    min: 10\n    max: 20";
    expect(check(int, "9")).toBe("Enter a number of at least 10.");
    expect(check(int, "21")).toBe("Enter a number of at most 20.");
    expect(check(int, "10")).toBeUndefined();
    expect(check(int, "20")).toBeUndefined();
    const float = "  - id: f\n    type: FLOAT\n    min: 0.1\n    max: 0.5";
    expect(check(float, "0.05")).toBe("Enter a number of at least 0.1.");
    expect(check(float, "0.6")).toBe("Enter a number of at most 0.5.");
    expect(check(float, "0.5")).toBeUndefined();
    // A malformed number keeps its own message, not a bound's.
    expect(check(int, "x")).toBe("Enter a whole number.");
  });

  it("DURATION: min and max compare the length, not the text", () => {
    const yaml = "  - id: d\n    type: DURATION\n    min: PT1H\n    max: P1D";
    // As text "PT30M" > "PT1H"; as a length it is shorter.
    expect(check(yaml, "PT30M")).toBe("Enter a duration of at least PT1H.");
    expect(check(yaml, "P1DT1S")).toBe("Enter a duration of at most P1D.");
    expect(check(yaml, "PT60M")).toBeUndefined();
    expect(check(yaml, "PT24H")).toBeUndefined();
    expect(check(yaml, "PT3600.5S")).toBeUndefined();
    expect(check(yaml, "-PT2H")).toBe("Enter a duration of at least PT1H.");
  });

  it("DATETIME, DATE and TIME: after and before, both inclusive", () => {
    const dt = '  - id: t\n    type: DATETIME\n    after: "2023-01-01T00:00:00Z"\n    before: "2023-12-31T23:59:59Z"';
    // The control holds local wall time; the bound is an instant.
    const local = (iso: string) => {
      const d = new Date(iso);
      const p = (n: number) => String(n).padStart(2, "0");
      return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
    };
    expect(check(dt, local("2022-12-31T23:59:59Z"))).toBe("Enter a date and time on or after 2023-01-01T00:00:00Z.");
    expect(check(dt, local("2024-01-01T00:00:00Z"))).toBe("Enter a date and time on or before 2023-12-31T23:59:59Z.");
    expect(check(dt, local("2023-01-01T00:00:00Z"))).toBeUndefined();
    const date = "  - id: d\n    type: DATE\n    after: 2023-01-01\n    before: 2023-12-31";
    expect(check(date, "2022-12-31")).toBe("Enter a date on or after 2023-01-01.");
    expect(check(date, "2024-01-01")).toBe("Enter a date on or before 2023-12-31.");
    expect(check(date, "2023-12-31")).toBeUndefined();
    const time = '  - id: t\n    type: TIME\n    after: "09:00"\n    before: "17:30:00"';
    expect(check(time, "08:59:59")).toBe("Enter a time on or after 09:00.");
    expect(check(time, "17:31")).toBe("Enter a time on or before 17:30:00.");
    // "9:00" < "10:00" holds as text only by luck; "17:30" and "17:30:00" are the same time.
    expect(check(time, "17:30")).toBeUndefined();
    expect(check(time, "09:00")).toBeUndefined();
  });

  it("leaves a bound it cannot read to the server", () => {
    expect(check("  - id: n\n    type: INT\n    min: abc", "1")).toBeUndefined();
    expect(check("  - id: d\n    type: DATE\n    after: soon", "2023-01-01")).toBeUndefined();
  });

  it("SELECT: a value outside `values` is refused when required, as the server does, unless allowCustomValue", () => {
    expect(check("  - id: s\n    type: SELECT\n    values: [a, b]", "z")).toBe("Choose one of the listed values; z is not one of them.");
    expect(check("  - id: s\n    type: SELECT\n    values: [a, b]", "b")).toBeUndefined();
    // Kestra checks a SELECT's values only when it is required (D4).
    expect(check("  - id: s\n    type: SELECT\n    values: [a, b]\n    required: false", "z")).toBeUndefined();
    expect(check("  - id: s\n    type: SELECT\n    values: [a, b]\n    allowCustomValue: true", "z")).toBeUndefined();
    // Values from an expression: there is no list to check against.
    expect(check('  - id: s\n    type: SELECT\n    values: "{{ x }}"', "z")).toBeUndefined();
  });

  it("MULTISELECT: every chosen value must be listed, unless allowCustomValue", () => {
    const yaml = "  - id: m\n    type: MULTISELECT\n    values: [a, b]\n    required: false";
    expect(check(yaml, ["a", "z", "y"])).toBe("Untick z, y: only the listed values are accepted.");
    expect(check(yaml, ["b", "a"])).toBeUndefined();
    expect(check(`${yaml}\n    allowCustomValue: true`, ["z"])).toBeUndefined();
  });
});

describe("validationRequestFromForm", () => {
  it("is the start's JSON body, with a chosen file as its name", () => {
    const list = inputs("  - id: data\n    type: FILE\n  - id: n\n    type: INT\n  - id: j\n    type: JSON");
    const file = new File(["a,b\n"], "rows.csv", { type: "text/csv" });
    const form = { ...formFromInputs(list), values: { data: file, n: "5", j: '"123"' } };
    expect(validationRequestFromForm(form)).toEqual({ inputs: { data: "rows.csv", n: 5, j: '"123"' } });
    // Without a file it is exactly the start's body.
    const noFile = set(form, "data", null);
    expect(validationRequestFromForm(noFile)).toEqual({ inputs: jsonInputs(noFile) });
  });
});

describe("validationView", () => {
  const list = () => inputs("  - id: n\n    type: INT\n  - id: m\n    type: MULTISELECT\n    values: [A]\n  - id: off\n    type: STRING\n  - id: grp\n    type: FORM");
  const outcome = (id: string, enabled: boolean, ...messages: string[]) => ({
    input: { id, type: "X" },
    value: null,
    enabled,
    isDefault: false,
    errors: messages.map((message) => ({ message, renderError: false })),
  });

  it("keeps the errors of enabled inputs the form shows, by id, and names the inputs dependsOn turns off", () => {
    const view = validationView(
      {
        id: "f",
        namespace: "n",
        checks: [],
        inputs: [
          outcome("n", true, "Invalid value for input `n`. Cause: it must be more than `10`"),
          outcome("m", true, "Invalid value for input `m`. Cause: value `D` doesn't match the values `[A]`\nInvalid value for input `m`. Cause: value `E` doesn't match the values `[A]`"),
          outcome("off", false, "Missing required input:off"),
          outcome("grp", true, "Invalid value for input `grp`. Cause: not supported"),
          outcome("ghost", true, "Missing required input:ghost"),
        ],
      },
      list(),
    );
    expect(view.errors).toEqual({
      n: "Invalid value for input `n`. Cause: it must be more than `10`",
      m: "Invalid value for input `m`. Cause: value `D` doesn't match the values `[A]`\nInvalid value for input `m`. Cause: value `E` doesn't match the values `[A]`",
    });
    expect(view.inactive).toEqual(["off"]);
  });

  it("answers no errors for a valid answer, or one of another shape", () => {
    expect(validationView({ id: "f", namespace: "n", checks: [], inputs: [outcome("n", true)] }, list())).toEqual({ errors: {}, inactive: [] });
    expect(validationView({} as never, list())).toEqual({ errors: {}, inactive: [] });
  });
});

describe("serverErrorsByInput", () => {
  const list = () => inputs("  - id: data\n    type: FILE\n  - id: n\n    type: INT\n  - id: m\n    type: MULTISELECT\n    values: [A]");
  it("finds the declared input a server message names", () => {
    const message = 'Invalid value for input `data`: "x.exe": File type not allowed.';
    expect(serverErrorsByInput(message, list())).toEqual({ fields: { data: message }, rest: [] });
    expect(serverErrorsByInput("Missing required input:n", list())).toEqual({ fields: { n: "Missing required input:n" }, rest: [] });
  });

  it("attaches each line of a multi-line refusal to the input it names, keeping lines of one input together", () => {
    const d = "Invalid value for input `m`. Cause: value `D` doesn't match the values `[A]`";
    const e = "Invalid value for input `m`. Cause: value `E` doesn't match the values `[A]`";
    const n = "Invalid value for input `n`. Cause: it must be more than `10`";
    expect(serverErrorsByInput([d, n, e].join("\n"), list())).toEqual({ fields: { m: `${d}\n${e}`, n }, rest: [] });
    // A line naming no input continues the one before it.
    expect(serverErrorsByInput(`${n}\n  and more`, list()).fields).toEqual({ n: `${n}\n  and more` });
  });

  it("answers nothing for a message naming no declared input, or none at all", () => {
    const zz = "the flow declares no input `zz`: each multipart part is named by an input id";
    expect(serverErrorsByInput(zz, list())).toEqual({ fields: {}, rest: [zz] });
    expect(serverErrorsByInput("Cannot execute flow: flow is disabled.", list()).fields).toEqual({});
    expect(serverErrorsByInput("Missing required input:zz", list()).fields).toEqual({});
  });
});

describe("serverErrorInput", () => {
  it("answers the first declared input a refusal names, or nothing", () => {
    const list = inputs("  - id: a\n    type: STRING\n  - id: b\n    type: STRING");
    expect(serverErrorInput("Missing required input:b\nInvalid value for input `a`. Cause: x", list)).toBe("b");
    expect(serverErrorInput("Cannot execute flow: flow is disabled.", list)).toBeUndefined();
  });
});
