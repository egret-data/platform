import { ExternalLink } from "lucide-react";
import type { ReactNode } from "react";

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
 */
export function ConfigTable({
  children,
  settingLabel = "Setting",
  valueClassName = "w-52",
}: {
  children: ReactNode;
  settingLabel?: string;
  /** Width utility for the Value column. Replaces the default. */
  valueClassName?: string;
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
      <TableBody>{children}</TableBody>
    </Table>
  );
}

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
 * `scope="colgroup"` and the `rowheader` role are what make it a heading rather
 * than a stray cell for anything not reading it visually.
 */
export function ConfigTableGroup({ label }: { label: string }) {
  return (
    <TableRow className="hover:bg-transparent">
      <TableCell
        colSpan={3}
        scope="colgroup"
        role="rowheader"
        className="pt-6 text-xs font-semibold uppercase tracking-wide text-muted-foreground"
      >
        {label}
      </TableCell>
    </TableRow>
  );
}

/**
 * One row of a {@link ConfigTable}: the label, a muted Details cell (help text
 * from `fieldHelp[fieldKey]` unless overridden, plus a docs link), and the edit
 * control passed as `children`. `details === null` forces an empty Details cell
 * even when the field has `fieldHelp`.
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
  mono,
  children,
}: {
  label: ReactNode;
  fieldKey?: string;
  htmlFor?: string;
  /** Override the Details text; `null` = force empty; omitted = fall back to fieldHelp. */
  details?: string | null;
  docsUrl?: string;
  mono?: boolean;
  children: ReactNode;
}) {
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
    detailText || unit || link ? (
      <>
        {detailText}
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
      {/* `max-w-prose` caps the TEXT, not the column. Details is the only
          unsized column, so under `table-fixed` it absorbs every pixel of
          surplus width the page has — on a wide monitor that would set this
          muted help as one line hundreds of characters long. The column may
          still take the width; the sentence stops at a readable measure. */}
      <TableCell className="hidden align-top whitespace-normal break-words text-sm text-muted-foreground lg:table-cell">
        {helpNode ? <div className="max-w-prose">{helpNode}</div> : null}
      </TableCell>
      <TableCell className="align-top">{children}</TableCell>
    </TableRow>
  );
}
