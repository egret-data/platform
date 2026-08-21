import { useRef, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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
 * The `:name` parameters a caller supplies on the query string — **one table
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
 * **`allowed` lives under Type, not in a column of its own.** It exists only
 * for `enum` — a sixth column would be empty on every other row, and under
 * `table-fixed` it would cost that width whether or not any row is an enum.
 * It sits in the Type cell because that is what it qualifies: it is the list
 * the type is drawn from, and reading down the Type column shows
 * `enum` with its members directly beneath, still inside the row it belongs
 * to. That row anchoring is the same reason it is not lifted out to a
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
                SQL as <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">:name</code>.
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
                  <Select value={p.type} onValueChange={(v) => update(i, { type: v as ParamType })}>
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
