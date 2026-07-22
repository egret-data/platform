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
import { fieldHelp } from "./fieldHelp";

/**
 * A `Setting | Details | Value` config table, mirroring the Storage/Warehouse
 * Configuration layout. Meant to sit inside a {@link ConfigSection} card, so it
 * renders borderless — the surrounding card is the frame. The Value column
 * holds live edit controls (the Settings tab is a single editing surface with
 * one Save), so there is no view/edit toggle here.
 */
export function ConfigTable({ children }: { children: ReactNode }) {
  return (
    // table-fixed so the (often long) Details help text wraps instead of
    // blowing the table past the viewport and pushing Value off-screen.
    <Table className="w-full table-fixed">
      <TableHeader>
        <TableRow>
          <TableHead className="w-48">Setting</TableHead>
          <TableHead className="hidden lg:table-cell">Details</TableHead>
          <TableHead className="w-64">Value</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>{children}</TableBody>
    </Table>
  );
}

/**
 * One row of a {@link ConfigTable}: the label, a muted Details cell (help text
 * from `fieldHelp[fieldKey]` unless overridden, plus a docs link), and the edit
 * control passed as `children`. `details === null` forces an empty Details cell
 * even when the field has `fieldHelp`.
 */
export function ConfigTableRow({
  label,
  fieldKey,
  htmlFor,
  details,
  docsUrl,
  children,
}: {
  label: string;
  fieldKey?: string;
  htmlFor?: string;
  /** Override the Details text; `null` = force empty; omitted = fall back to fieldHelp. */
  details?: string | null;
  docsUrl?: string;
  children: ReactNode;
}) {
  const mapped = fieldKey ? fieldHelp[fieldKey] : undefined;
  const detailText = details === undefined ? mapped?.help : details;
  const unit = mapped?.unit;
  const link = docsUrl ?? mapped?.docsUrl;

  return (
    <TableRow>
      <TableCell className="align-top font-medium whitespace-normal break-words">
        {htmlFor ? <label htmlFor={htmlFor}>{label}</label> : label}
      </TableCell>
      <TableCell className="hidden align-top whitespace-normal break-words text-sm text-muted-foreground lg:table-cell">
        {detailText}
        {unit ? <span className="text-muted-foreground/70"> ({unit})</span> : null}
        {link ? (
          <a
            href={link}
            target="_blank"
            rel="noreferrer"
            aria-label={`${label} documentation`}
            className="ml-1 inline-flex text-muted-foreground hover:text-foreground"
          >
            <ExternalLink className="size-3.5" />
          </a>
        ) : null}
      </TableCell>
      <TableCell className="align-top">{children}</TableCell>
    </TableRow>
  );
}
