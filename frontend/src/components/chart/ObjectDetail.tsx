import { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { formatValue } from "@/components/chart/format";
import type { ColumnFormat } from "@/components/chart/format-types";
import type { ResultColumn } from "@/components/chart/ChartTable";

/** Past this many characters a value is folded behind a "Show more" — Metabase's `ExpandableString`
 *  length. A detail view is a column of short rows; one 4,000-character note otherwise pushes every
 *  other field off the card. */
const EXPAND_AT = 140;

/** A value that renders as an image rather than as its URL. Metabase gates on the column's semantic
 *  type (`isImageURL`/`isAvatarURL`); egret has no semantic types on a result column, so it gates on
 *  the value itself — an http(s) URL ending in an image extension. Narrower than Metabase's, and
 *  deliberately so: guessing wrong here replaces a readable URL with a broken-image icon. */
const IMAGE_URL = /^https?:\/\/\S+\.(png|jpe?g|gif|webp|avif|svg)(\?\S*)?$/i;

/** Pretty-printed when a value is JSON. Metabase checks the column's `SerializedJSON` semantic type
 *  and also catches an object the driver already parsed; egret sees only the value, so it tries to
 *  parse anything that starts like an object or array. A string that merely looks like one and does
 *  not parse falls through unchanged rather than being mangled. */
function asJson(value: unknown): string | undefined {
  if (value != null && typeof value === "object") return JSON.stringify(value, null, 2);
  if (typeof value !== "string") return undefined;
  const t = value.trim();
  if (!(t.startsWith("{") && t.endsWith("}")) && !(t.startsWith("[") && t.endsWith("]"))) return undefined;
  try {
    return JSON.stringify(JSON.parse(t), null, 2);
  } catch {
    return undefined;
  }
}

function Value({ value, column, format }: {
  value: unknown; column: ResultColumn; format?: ColumnFormat;
}) {
  const [expanded, setExpanded] = useState(false);

  // Metabase prints the word "Empty" in a muted tone. egret rendered "" — a blank row that reads as
  // a rendering fault rather than as an absent value, which is the one thing a detail view of a
  // single record most needs to say clearly.
  if (value == null || value === "") {
    return <span className="text-muted-foreground/70">Empty</span>;
  }

  const json = asJson(value);
  if (json != null) {
    return (
      <pre className="max-h-72 overflow-auto whitespace-pre-wrap rounded bg-muted/50 p-2 text-xs leading-relaxed">
        {json}
      </pre>
    );
  }

  if (typeof value === "string" && IMAGE_URL.test(value)) {
    return <img src={value} alt={value} className="mx-auto my-1 max-h-72 max-w-full rounded" />;
  }

  const text = formatValue(value, column.type, format);
  if (text.length <= EXPAND_AT) return <>{text}</>;
  return (
    <>
      {expanded ? text : `${text.slice(0, EXPAND_AT)}…`}{" "}
      <button
        type="button"
        className="text-xs font-medium text-primary underline underline-offset-2"
        onClick={() => setExpanded((v) => !v)}
      >
        {expanded ? "Show less" : "Show more"}
      </button>
    </>
  );
}

/** Renders a single record as a vertical field/value list, paging through the rows. */
export function ObjectDetail({
  columns,
  rows,
  columnSettings = {},
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  columnSettings?: Record<string, ColumnFormat>;
}) {
  const [index, setIndex] = useState(0);
  // Reset to the first record whenever the data changes (a new query → new rows reference).
  useEffect(() => setIndex(0), [rows]);

  if (rows.length === 0) {
    return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>;
  }
  const i = Math.min(index, rows.length - 1);
  const row = rows[i];

  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1 overflow-auto">
        <dl className="divide-y">
          {columns.map((c, ci) => (
            <div key={c.name} className="flex gap-3 px-3 py-2">
              <dt className="w-2/5 shrink-0 truncate text-xs font-medium uppercase tracking-wide text-muted-foreground" title={c.name}>{c.name}</dt>
              <dd className="min-w-0 flex-1 break-words text-sm tabular-nums">
                {/* Keyed by the record as well as the column: paging to the next record must
                    collapse an expanded value rather than carry its state onto a different one. */}
                <Value key={`${i}:${c.name}`} value={row[ci]} column={c} format={columnSettings[c.name]} />
              </dd>
            </div>
          ))}
        </dl>
      </div>
      {rows.length > 1 && (
        <div className="flex items-center justify-between border-t px-3 py-1.5 text-xs text-muted-foreground">
          <button type="button" aria-label="previous record" disabled={i === 0}
            onClick={() => setIndex(i - 1)} className="disabled:opacity-40">
            <ChevronLeft className="h-4 w-4" />
          </button>
          <span>{i + 1} / {rows.length}</span>
          <button type="button" aria-label="next record" disabled={i >= rows.length - 1}
            onClick={() => setIndex(i + 1)} className="disabled:opacity-40">
            <ChevronRight className="h-4 w-4" />
          </button>
        </div>
      )}
    </div>
  );
}
