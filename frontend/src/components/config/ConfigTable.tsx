import { ExternalLink, Info } from "lucide-react";
import { createContext, useContext } from "react";
import type { ReactNode } from "react";

import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
// `fieldHelp` is an engine-specific catalog (Trino/StarRocks config keys), so
// it stays under control-plane rather than moving here with the table shell.
// Callers outside control-plane (e.g. Data API) don't pass `fieldKey` and
// supply `details` literals directly instead.
import { fieldHelp } from "@/routes/control-plane/config/fieldHelp";

/**
 * A `Setting | Details | Value` config table, mirroring the Storage/Warehouse
 * Configuration layout. Meant to sit inside a {@link ConfigSection} card, so it
 * renders borderless — the surrounding card is the frame. The Value column
 * holds live edit controls (the Settings tab is a single editing surface with
 * one Save), so there is no view/edit toggle here.
 *
 * `settingLabel` renames the first column — property-map editors pass
 * "Property" so the key column reads correctly.
 *
 * `valueClassName` widens (or narrows) the Value column for a caller whose
 * controls need more room than the shared 208px. It has to live here rather
 * than in the caller's cell: under `table-layout: fixed` a column is exactly
 * as wide as its `<th>` says, so a `w-96` wrapper inside a `w-52` cell
 * overflows into its neighbour instead of widening the column. The default
 * keeps every existing consumer byte-identical.
 *
 * `detailsClassName` wraps each row's Details content in a div carrying that
 * class — for a caller whose page has no width cap, where `table-fixed` hands
 * ALL surplus width to Details (the only unsized column) and a help sentence
 * can set hundreds of characters wide. Opt-in for the same reason: eleven other
 * consumers render this cell today and none of them were measured, so the
 * default emits no wrapper at all and their markup is unchanged.
 */
export function ConfigTable({
  children,
  settingLabel = "Setting",
  valueClassName = "w-52",
  detailsClassName,
}: {
  children: ReactNode;
  settingLabel?: string;
  /** Width utility for the Value column. Replaces the default. */
  valueClassName?: string;
  /** Class for a wrapper around each Details cell's content. Omitted = no wrapper. */
  detailsClassName?: string;
}) {
  return (
    // table-fixed so the (often long) Details help text wraps instead of
    // blowing the table past the viewport and pushing Value off-screen.
    <Table className="w-full table-fixed">
      <TableHeader>
        <TableRow>
          <TableHead className="w-72">{settingLabel}</TableHead>
          <TableHead className="hidden lg:table-cell">Details</TableHead>
          <TableHead className={valueClassName}>Value</TableHead>
        </TableRow>
      </TableHeader>
      {/* Through context rather than a prop on every `ConfigTableRow`: this is
          column geometry, which the table owns, and Config alone has twenty
          rows that would each have to repeat it (and could each forget it). */}
      <ConfigTableContext.Provider value={{ detailsClassName }}>
        <TableBody>{children}</TableBody>
      </ConfigTableContext.Provider>
    </Table>
  );
}

/** Column geometry `ConfigTable` sets and `ConfigTableRow` reads. */
const ConfigTableContext = createContext<{ detailsClassName?: string }>({});

/**
 * A full-width heading row inside a `ConfigTable`.
 *
 * The Data API detail sections used to stack one `ConfigSection` card per group,
 * each framing a table of one or two rows — read down the page, a column of
 * mostly-empty boxes. Groups belong inside the table instead, and a group row
 * has to span all three columns: one that occupied only the Setting column
 * would read as a setting with no value, which is the confusion the cards were
 * replaced to remove.
 *
 * `role="rowheader"` is what makes it a heading rather than a stray cell for
 * anything not reading it visually — and it is the whole of it. This cell also
 * carried `scope="colgroup"`, which was removed: `scope` is a `<th>`-only
 * attribute and does nothing on a `<td>`. The branch's own mutation said so
 * before anyone read the spec — dropping `role` while KEEPING `scope` still
 * kills four tests, so `scope` was contributing nothing. It stays a `<td>` with
 * an explicit role rather than becoming a `<th scope="colgroup">`, because a
 * `<th>` brings its own implicit role to argue with.
 */
export function ConfigTableGroup({ label }: { label: string }) {
  return (
    <TableRow className="hover:bg-transparent">
      <TableCell
        colSpan={3}
        role="rowheader"
        className="pt-6 text-xs font-semibold uppercase tracking-wide text-muted-foreground"
      >
        {label}
      </TableCell>
    </TableRow>
  );
}

/**
 * The rest of a row's explanation, one click away: Details holds one short
 * line (KI-219), and what does not fit in it opens here. A popover rather
 * than a hover tooltip, so it opens by keyboard and on touch as well.
 */
function MoreDetails({ label, text }: { label: ReactNode; text: string }) {
  return (
    <Popover>
      <PopoverTrigger
        type="button"
        aria-label={typeof label === "string" ? `More about ${label}` : "More details"}
        className="ml-1 inline-flex size-5 translate-y-0.5 items-center justify-center rounded text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        <Info className="size-3.5" aria-hidden />
      </PopoverTrigger>
      <PopoverContent align="start" className="w-80 text-xs leading-relaxed">
        {text}
      </PopoverContent>
    </Popover>
  );
}

/**
 * One row of a {@link ConfigTable}: the label, a muted Details cell (help text
 * from `fieldHelp[fieldKey]` unless overridden, plus a docs link), and the edit
 * control passed as `children`. `details === null` forces an empty Details cell
 * even when the field has `fieldHelp`.
 *
 * `more` is the long form of `details`: keep `details` to one line and put
 * the rest here, behind an info button.
 *
 * `mono` renders a string label in monospace — used when the label is the exact
 * config key (e.g. `query.max-memory`) so it's easy to look up in the docs.
 */
export function ConfigTableRow({
  label,
  fieldKey,
  htmlFor,
  details,
  docsUrl,
  more,
  mono,
  children,
}: {
  label: ReactNode;
  fieldKey?: string;
  htmlFor?: string;
  /** Override the Details text; `null` = force empty; omitted = fall back to fieldHelp. */
  details?: string | null;
  docsUrl?: string;
  /** The rest of the explanation, behind an info button after Details. */
  more?: string;
  mono?: boolean;
  children: ReactNode;
}) {
  const { detailsClassName } = useContext(ConfigTableContext);
  const mapped = fieldKey ? fieldHelp[fieldKey] : undefined;
  const detailText = details === undefined ? mapped?.help : details;
  const unit = mapped?.unit;
  const link = docsUrl ?? mapped?.docsUrl;

  const labelInner =
    mono && typeof label === "string" ? (
      <span className="break-all font-mono text-xs">{label}</span>
    ) : (
      label
    );

  // Help text + unit + docs link. Shown in the Details column at lg+, and
  // (below lg, where the Details column is hidden) inline under the Setting
  // label — so the help and the docs link are never lost on narrow screens.
  const helpNode =
    detailText || unit || link || more ? (
      <>
        {detailText}
        {more ? <MoreDetails label={label} text={more} /> : null}
        {unit ? <span className="text-muted-foreground/70"> ({unit})</span> : null}
        {link ? (
          <a
            href={link}
            target="_blank"
            rel="noreferrer"
            aria-label="documentation"
            className="ml-1 inline-flex text-muted-foreground hover:text-foreground"
          >
            <ExternalLink className="size-3.5" />
          </a>
        ) : null}
      </>
    ) : null;

  return (
    <TableRow>
      <TableCell className="align-top font-medium whitespace-normal break-words">
        {htmlFor ? <label htmlFor={htmlFor}>{labelInner}</label> : labelInner}
        {helpNode ? (
          <div className="mt-1 text-xs font-normal text-muted-foreground lg:hidden">
            {helpNode}
          </div>
        ) : null}
      </TableCell>
      {/* The wrapper caps the TEXT, not the column: Details is the only unsized
          column, so under `table-fixed` it absorbs every pixel of surplus width
          an uncapped page has, and the column may keep that width while the
          sentence stops at a readable measure. Emitted only when the table asked
          for it — with no `detailsClassName` this renders exactly what it always
          did, no wrapper element at all. */}
      <TableCell className="hidden align-top whitespace-normal break-words text-sm text-muted-foreground lg:table-cell">
        {detailsClassName && helpNode ? <div className={detailsClassName}>{helpNode}</div> : helpNode}
      </TableCell>
      {/* `whitespace-normal` is load-bearing here, not defensive symmetry with
          the two cells above. The shared `TableCell` is `whitespace-nowrap`
          (`ui/table.tsx`), and the Value column is the narrowest one this table
          sizes — 208px by default, 384px at `TryIt`'s `w-52 lg:w-96`. A caller
          may legitimately put a sentence in it: `TryIt` puts three there (the
          failed-fetch `Alert`, the no-service-accounts note, and a `Caveat`'s
          disclosure body), because each is about the picker in this cell and
          §4.3 keeps an explanation beside the control it is about.

          Measured in Chromium at 1440px before this line existed, with the
          Value column at 304px: the caveat body laid out at 1103px, the empty
          note at 498px and the alert at 807px — clipped with no ellipsis
          (`text-overflow` is `clip`), and the whole `Setting | Details | Value`
          table overflowed its `overflow-x-auto` wrapper (1807, 1202 and 1512
          against 1016), putting a horizontal scrollbar under a settings table
          on a desktop viewport. Two of the three need nothing clicked.

          It belongs HERE and not in the callers: an earlier pass fixed the same
          inheritance on `Caveat`'s `<summary>` alone, and every other block in
          the same cell kept clipping. `break-words` for the other half — a
          server error message this cell renders verbatim can be one
          unbreakable token, and wrapping cannot help a word with no break
          opportunity in it. Controls are unaffected: `Button`, `Badge` and the
          `Select` trigger all carry their own `whitespace-nowrap`. */}
      <TableCell className="align-top whitespace-normal break-words">{children}</TableCell>
    </TableRow>
  );
}
