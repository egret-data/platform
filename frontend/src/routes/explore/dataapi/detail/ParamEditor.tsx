import { useRef, useState, type ComponentProps } from "react";
import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Checkbox } from "@/components/ui/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { ParamLocation, ParamType, type ParamSpec } from "@/api/generated/model";

const PARAM_TYPES: ParamType[] = [
  ParamType.string,
  ParamType.number,
  ParamType.boolean,
  ParamType.date,
  ParamType.timestamp,
  ParamType.enum,
];

function newParam(): ParamSpec {
  // Every parameter is `query` — there is no location control here. See the
  // module doc below for why offering one would be worse than omitting it.
  return { name: "", type: ParamType.string, location: ParamLocation.query, required: false };
}

/** The seven optional constraint fields `check_constraints` validates at
 *  publish time — see `ParamSpec`'s generated doc comments for what each one
 *  means and why `minimum`/`maximum` are strings. */
const CONSTRAINT_FIELDS = [
  "minimum",
  "maximum",
  "min_length",
  "max_length",
  "pattern",
  "not_before",
  "not_after",
] as const;
type ConstraintField = (typeof CONSTRAINT_FIELDS)[number];

/** Which constraint fields apply to a given parameter type — the same
 *  pairing the backend's `ConstraintNotForType` enforces at publish time.
 *  `boolean` and `enum` take none: `allowed` is already the enum's
 *  constraint, and the type is the boolean's. */
function constraintFieldsFor(type: ParamType): ConstraintField[] {
  switch (type) {
    case ParamType.string:
      return ["min_length", "max_length", "pattern"];
    case ParamType.number:
      return ["minimum", "maximum"];
    case ParamType.date:
    case ParamType.timestamp:
      return ["not_before", "not_after"];
    default:
      return [];
  }
}

/** A patch clearing every constraint field the new type does not use.
 *  Without this, a `max_length` set while the row was `string` survives a
 *  switch to `number` and publish refuses the whole spec with
 *  `ConstraintNotForType` — an error about a field the editor no longer
 *  shows the author, since it is scoped to `newType`'s applicable set.
 *
 *  `allowed` is cleared on the same rule and for the same reason. It is not in
 *  `CONSTRAINT_FIELDS` because it is a `string[]` rather than one of the seven
 *  scalar fields, but the editor shows it for `enum` alone — so an `enum`
 *  switched to `string` used to carry an invisible `allowed` forever. That was
 *  harmless while the backend ignored it; since it refuses it
 *  (`ConstraintNotForType`, package 5 review), an uncleared list would be a
 *  publish the author cannot fix from this screen. */
function clearInapplicableConstraints(newType: ParamType): Partial<ParamSpec> {
  const keep = new Set<ConstraintField>(constraintFieldsFor(newType));
  const patch: Partial<ParamSpec> = {};
  for (const field of CONSTRAINT_FIELDS) {
    if (!keep.has(field)) patch[field] = undefined;
  }
  if (newType !== ParamType.enum) patch.allowed = undefined;
  return patch;
}

/**
 * The comma-separated "allowed values" field for an `enum` parameter.
 *
 * Deliberately uncontrolled between keystrokes: an earlier version was
 * `value={allowed.join(", ")}` with an `onChange` that immediately re-split,
 * trimmed and `filter(Boolean)`-ed on every keystroke. That normalizes a
 * trailing comma away the instant it's typed — existing value `"a"`, type
 * `,` → `"a,"` → split/trim/filter → back to `["a"]` → rendered as `"a"` →
 * the keystroke is silently eaten, and a second value can never be started
 * by typing. This field instead holds the raw text locally and only projects
 * it to `string[]` `onBlur`, which is also where it re-formats the display
 * (`"a,b"` → `"a, b"`) so a typo in spacing doesn't linger once the field is
 * left.
 *
 * The local copy is a DRAFT, not a mirror: `draft === null` means "showing
 * whatever `allowed` currently is", and only a keystroke puts a string in it.
 * That is what keeps it from going stale. `EndpointDetail` re-seeds the whole
 * form from the SERVER RESPONSE after every Save, so a mirror seeded once at
 * mount would still show the pre-save string after a server-normalised
 * `allowed` came back — and the next focus/blur would `commit()` that stale
 * text straight back over what the server just returned. With a draft,
 * `commit()` clears it, the field falls back to the committed prop, and a
 * focus/blur that typed nothing commits nothing at all.
 *
 * Note what this deliberately is NOT: a `useEffect` that calls `setText` when
 * the prop changes (the `react-hooks/set-state-in-effect` shape this file
 * avoids everywhere else), and not a `key` on the field either — a key would
 * remount the input and hand back a different DOM node on every commit.
 */
function AllowedValuesField({
  id,
  label,
  allowed,
  onChange,
}: {
  id: string;
  /** Accessible name. Carries the row's number, like every other cell's — the
   *  visible column header cannot say WHICH row a control belongs to. */
  label: string;
  allowed: string[];
  onChange: (allowed: string[]) => void;
}) {
  // `null` = show the committed prop; a string = the author is mid-edit.
  const [draft, setDraft] = useState<string | null>(null);
  const text = draft ?? allowed.join(", ");

  function commit() {
    // Nothing was typed since the last commit, so there is nothing to project
    // — and, crucially, no stale local text to push back over the prop.
    if (draft === null) return;
    const projected = draft
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    // Drop the draft FIRST: the field then renders the committed value, which
    // this `onChange` is about to make `projected` — that is what re-formats
    // `"a,b"` to `"a, b"`, without a second copy of it to keep in sync.
    setDraft(null);
    onChange(projected);
  }

  return (
    <Input
      id={id}
      aria-label={label}
      placeholder="comma-separated"
      className="mt-1.5"
      value={text}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
    />
  );
}

/**
 * One constraint input with its own visible label, tied by `htmlFor`/`id`
 * (FE-FORM-2). The label used to be the placeholder alone, which disappears
 * the moment the box is filled — a number parameter then showed `0` and
 * `1000000` under its type with nothing saying which bound was which (KI-026).
 * The `aria-label` stays: it carries the ROW ("Parameter 2 minimum"), which a
 * label above one input in one cell cannot.
 */
function ConstraintInput({
  id,
  label,
  ariaLabel,
  ...input
}: {
  id: string;
  label: string;
  ariaLabel: string;
} & Omit<ComponentProps<typeof Input>, "id" | "aria-label">) {
  return (
    <div className="space-y-1">
      <Label htmlFor={id} className="text-xs font-normal text-muted-foreground">
        {label}
      </Label>
      <Input id={id} aria-label={ariaLabel} {...input} />
    </div>
  );
}

/**
 * The constraint inputs for `string`, `number`, `date` and `timestamp` — the
 * types that have any (see `constraintFieldsFor`). Each carries a visible
 * label and an `aria-label` naming its row (see `ConstraintInput`), and a
 * plain controlled value straight off `p` with no local draft. The draft trick
 * `AllowedValuesField` needs exists because it reformats text on blur
 * (`"a,b"` → `"a, b"`); nothing here reformats what the author typed, so there
 * is no re-render to fight.
 */
function ConstraintFields({
  id,
  row,
  param,
  onChange,
}: {
  id: string;
  /** Row label, e.g. "Parameter 2" — carried into every input's `aria-label`
   *  for the same reason every other cell's controls do (see the table's
   *  module doc: a visible `<th>` cannot say which row a control belongs to). */
  row: string;
  param: ParamSpec;
  onChange: (patch: Partial<ParamSpec>) => void;
}) {
  switch (param.type) {
    case ParamType.string:
      return (
        <div className="mt-1.5 space-y-1.5">
          <ConstraintInput
            id={`${id}-min-length`}
            label="Min length"
            ariaLabel={`${row} min length`}
            type="number"
            min={0}
            value={param.min_length ?? ""}
            onChange={(e) =>
              onChange({ min_length: e.target.value === "" ? undefined : Number(e.target.value) })
            }
          />
          <ConstraintInput
            id={`${id}-max-length`}
            label="Max length"
            ariaLabel={`${row} max length`}
            type="number"
            min={0}
            value={param.max_length ?? ""}
            onChange={(e) =>
              onChange({ max_length: e.target.value === "" ? undefined : Number(e.target.value) })
            }
          />
          <ConstraintInput
            id={`${id}-pattern`}
            label="Pattern (regex)"
            ariaLabel={`${row} pattern`}
            value={param.pattern ?? ""}
            onChange={(e) => onChange({ pattern: e.target.value === "" ? undefined : e.target.value })}
          />
        </div>
      );
    case ParamType.number:
      return (
        // Text inputs, deliberately not `type="number"` — `minimum`/`maximum`
        // are strings on the wire because a decimal that round-trips through
        // a float stops comparing equal to itself on the engine, and a
        // numeric input would round-trip it through the browser's float too.
        <div className="mt-1.5 space-y-1.5">
          <ConstraintInput
            id={`${id}-minimum`}
            label="Minimum"
            ariaLabel={`${row} minimum`}
            value={param.minimum ?? ""}
            onChange={(e) => onChange({ minimum: e.target.value === "" ? undefined : e.target.value })}
          />
          <ConstraintInput
            id={`${id}-maximum`}
            label="Maximum"
            ariaLabel={`${row} maximum`}
            value={param.maximum ?? ""}
            onChange={(e) => onChange({ maximum: e.target.value === "" ? undefined : e.target.value })}
          />
        </div>
      );
    case ParamType.date:
    case ParamType.timestamp:
      return (
        <div className="mt-1.5 space-y-1.5">
          <ConstraintInput
            id={`${id}-not-before`}
            label="Not before"
            ariaLabel={`${row} not before`}
            value={param.not_before ?? ""}
            onChange={(e) => onChange({ not_before: e.target.value === "" ? undefined : e.target.value })}
          />
          <ConstraintInput
            id={`${id}-not-after`}
            label="Not after"
            ariaLabel={`${row} not after`}
            value={param.not_after ?? ""}
            onChange={(e) => onChange({ not_after: e.target.value === "" ? undefined : e.target.value })}
          />
        </div>
      );
    default:
      return null;
  }
}

/**
 * The `{{ name }}` parameters a caller supplies on the query string — **one table
 * row per declaration**, not one card per declaration.
 *
 * The card-per-parameter stack this replaced put a `grid grid-cols-2` inside a
 * bordered box for every row: five fields read as five separate forms, and two
 * parameters could not be compared without scrolling past the box between them.
 * `docs/en/ui-conventions.md` §4 records the same complaint about the detail
 * sections generally, and the same answer — a group of like things is a table.
 * `Setting | Details | Value` (`components/config/ConfigTable.tsx`) is the
 * shape for a list of *settings*, each with its own help text; a parameter is
 * not a setting but a record with five fields, so this is a plain
 * `components/ui/table` with one column per field, following that file's
 * conventions (`table-fixed` so the sized columns hold and the surplus goes to
 * one chosen column, `align-top` cells, the shadcn `Table`'s own
 * `overflow-x-auto` wrapper for narrow viewports) rather than its component.
 *
 * **Column identity is what makes a row editable.** A visible `<th>` labels a
 * column, never a particular row's control, so every control carries an
 * `aria-label` naming its row — `Parameter 2 default`. Without it, five inputs
 * per row would be five unnamed text boxes to anything not reading the table
 * visually, and a test could not tell an edit to the right row from an edit to
 * the wrong one.
 *
 * **Deliberately no `location` control** (spec §2.6). Every parameter this API
 * can serve is `query`: `ParamLocation::Body` is refused at publish time, and
 * `serve_post` (the POST handler) has no body extractor to read one from. A
 * picker that offers `body` and then has publish reject it is a control that
 * can drift out of sync with the refusal it's describing — omitting the
 * control keeps the two impossible to disagree.
 *
 * **`allowed`, and the seven constraint fields, live under Type, not in a
 * column of their own.** Between them these cover every type but `boolean`
 * — a column each would mean up to seven more columns, empty on every row
 * whose type doesn't use them, and under `table-fixed` each would cost that
 * width whether or not any row needs it. They sit in the Type cell because
 * that is what they qualify: `allowed` is the list `enum` draws from,
 * `min_length`/`max_length`/`pattern` bound a `string`, `minimum`/`maximum`
 * bound a `number`, `not_before`/`not_after` bound a `date` or `timestamp`
 * — and reading down the Type column shows each type with its own
 * constraints directly beneath, still inside the row they belong to. That
 * row anchoring is the same reason none of this is lifted out to a
 * per-table area below.
 *
 * **No test value here, and this is the one field the table must NOT gain.**
 * `PreviewBlock` holds a test value per declared parameter; it is component
 * state and is never saved. A test value in a declaration row invites the
 * reader to think it is stored with the endpoint (see `Query.tsx`'s comment).
 *
 * Rows are keyed by a stable, locally-generated id — never the array index.
 * `ParamSpec` has no server-assigned id to key on, but an index breaks the
 * moment a row in the middle is removed: React re-associates each surviving
 * row's DOM node with the NEXT row's data, which just relocates a controlled
 * `<Input>`'s value (it's fully re-rendered either way) but leaves a Radix
 * `Select`'s open/focus state and this file's own `AllowedValuesField` local
 * text attached to the wrong row. The whole editor also remounts fresh
 * (`EndpointDetail` keys the `Outlet` by endpoint id) whenever the author
 * navigates to a different endpoint, so these ids never need to survive that.
 */
export function ParamEditor({
  params,
  onChange,
}: {
  params: ParamSpec[];
  onChange: (params: ParamSpec[]) => void;
}) {
  // Existing rows keep their index as a stable id for the component's whole
  // lifetime — assigned once here, at mount, and never regenerated from
  // `params` again. `nextRowId` only hands out ids for rows added AFTER
  // mount (via `add`, an event handler — mutating a ref there is fine; doing
  // it inside a `useState` lazy initializer is not, since that runs during
  // render). Starting it at `params.length` guarantees a new row's id can
  // never collide with one of the initial ones.
  const nextRowId = useRef(params.length);
  const [rowIds, setRowIds] = useState<number[]>(() => params.map((_, i) => i));

  function update(i: number, patch: Partial<ParamSpec>) {
    onChange(params.map((p, j) => (j === i ? { ...p, ...patch } : p)));
  }
  function remove(i: number) {
    onChange(params.filter((_, j) => j !== i));
    setRowIds((ids) => ids.filter((_, j) => j !== i));
  }
  function add() {
    const rowId = nextRowId.current++;
    onChange([...params, newParam()]);
    setRowIds((ids) => [...ids, rowId]);
  }

  return (
    <div className="space-y-3">
      <h3 className="text-sm font-medium">Parameters</h3>
      {/* `table-fixed` for the same reason `ConfigTable` uses it: under auto
          layout every column sizes to the intrinsic width of the input inside
          it, so the five columns fight each other and the table can outgrow the
          page. Sized here except Description, which is therefore the column
          that absorbs whatever width is left — the right choice on a section
          with no width cap, since a description is the one field that reads
          better long. */}
      <Table className="table-fixed">
        <TableHeader>
          <TableRow>
            <TableHead className="w-1/5">Name</TableHead>
            <TableHead className="w-44">Type</TableHead>
            <TableHead className="w-24">Required</TableHead>
            <TableHead className="w-1/6">Default</TableHead>
            <TableHead>Description</TableHead>
            {/* A header the column still needs — an empty `<th>` leaves the
                remove buttons in a column with no accessible name at all. */}
            <TableHead className="w-12">
              <span className="sr-only">Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {params.length === 0 && (
            <TableRow className="hover:bg-transparent">
              {/* Spanning every column, so the sentence reads as the state of
                  the whole table rather than as a value in the Name column. */}
              <TableCell colSpan={6} className="text-sm text-muted-foreground">
                No parameters yet. Every parameter is read from the query string — write it into the
                SQL as{" "}
                <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">{"{{ name }}"}</code>.
              </TableCell>
            </TableRow>
          )}
          {params.map((p, i) => {
            const rowId = rowIds[i] ?? i;
            // Numbered by POSITION, not by `p.name`: a name is blank on a
            // freshly-added row and is not unique while the author is editing,
            // so it cannot name a control. Same reason `PreviewBlock` keys its
            // test values by index.
            const row = `Parameter ${i + 1}`;
            return (
              <TableRow key={rowId}>
                <TableCell className="align-top">
                  <Input
                    id={`param-name-${rowId}`}
                    aria-label={`${row} name`}
                    value={p.name}
                    onChange={(e) => update(i, { name: e.target.value })}
                  />
                </TableCell>
                <TableCell className="align-top">
                  <Select
                    value={p.type}
                    onValueChange={(v) => {
                      const type = v as ParamType;
                      // Clear whatever constraints the OLD type left set that
                      // the new type doesn't use, in the same `update` that
                      // changes the type — see `clearInapplicableConstraints`.
                      update(i, { type, ...clearInapplicableConstraints(type) });
                    }}
                  >
                    <SelectTrigger id={`param-type-${rowId}`} className="w-full" aria-label={`${row} type`}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {PARAM_TYPES.map((t) => (
                        <SelectItem key={t} value={t}>
                          {t}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  {/* Only for `enum`, and only inside the row whose type it
                      qualifies — see the module doc for why it is here rather
                      than in a column of its own. */}
                  {p.type === ParamType.enum && (
                    <AllowedValuesField
                      id={`param-allowed-${rowId}`}
                      label={`${row} allowed values`}
                      allowed={p.allowed ?? []}
                      onChange={(allowed) => update(i, { allowed })}
                    />
                  )}
                  <ConstraintFields
                    id={`param-constraint-${rowId}`}
                    row={row}
                    param={p}
                    onChange={(patch) => update(i, patch)}
                  />
                </TableCell>
                <TableCell className="align-top">
                  {/* `mt-2.5` rather than centred: the row's height is set by
                      the inputs beside it, and a box that centres itself in a
                      row an enum has made two controls tall floats away from
                      the header it answers to. */}
                  <Checkbox
                    id={`param-required-${rowId}`}
                    className="mt-2.5"
                    aria-label={`${row} required`}
                    checked={!!p.required}
                    onCheckedChange={(v) => update(i, { required: v === true })}
                  />
                </TableCell>
                <TableCell className="align-top">
                  <Input
                    id={`param-default-${rowId}`}
                    aria-label={`${row} default`}
                    value={p.default ?? ""}
                    onChange={(e) => update(i, { default: e.target.value === "" ? null : e.target.value })}
                  />
                </TableCell>
                <TableCell className="align-top">
                  <Input
                    id={`param-desc-${rowId}`}
                    aria-label={`${row} description`}
                    value={p.description ?? ""}
                    onChange={(e) => update(i, { description: e.target.value })}
                  />
                </TableCell>
                <TableCell className="align-top">
                  {/* Named for the row, not "Remove": six identical buttons in a
                      column are six identical announcements otherwise, and the
                      icon carries no text of its own. */}
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    aria-label={`Remove ${row.toLowerCase()}`}
                    onClick={() => remove(i)}
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
      <Button type="button" variant="outline" size="sm" onClick={add}>
        <Plus className="h-4 w-4" /> Add parameter
      </Button>
    </div>
  );
}
