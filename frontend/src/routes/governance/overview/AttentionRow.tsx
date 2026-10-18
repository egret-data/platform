import { Link } from "react-router-dom";
import { AlertOctagon, AlertTriangle, Info, type LucideIcon } from "lucide-react";
import type { Finding, Ref, Severity } from "@/api/generated/model";
import { refUrl } from "@/routes/governance/overview/refUrl";

/** The shared focus ring (FE-A11Y-2) for an inline link. */
export const LINK = "rounded-sm text-primary hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";

/**
 * The colour is on the icon; the word stays in the text colour, so it reads in
 * light and dark alike (live check D15: the warning word in `highlight-foreground`
 * all but vanished on the dark background). The word and the icon's shape carry
 * the severity, never the colour alone (FE-A11Y-4).
 */
export const SEVERITY: Record<Severity, { word: string; icon: LucideIcon; tone: string }> = {
  critical: { word: "Critical", icon: AlertOctagon, tone: "text-destructive" },
  warning: { word: "Warning", icon: AlertTriangle, tone: "text-highlight" },
  info: { word: "Info", icon: Info, tone: "text-muted-foreground" },
};

function RefLink({ r, children }: { r: Ref | null | undefined; children: string }) {
  const url = r ? refUrl(r) : null;
  return url ? (
    <Link to={url} className={LINK}>
      {children}
    </Link>
  ) : (
    <span>{children}</span>
  );
}

/**
 * One finding: the severity as a word and an icon (never colour alone,
 * FE-A11Y-4), the server's sentence, up to five samples each with its own
 * link, and "Open" to where it is fixed.
 */
export function AttentionRow({ finding }: { finding: Finding }) {
  const sev = SEVERITY[finding.severity];
  const Icon = sev.icon;
  const open = finding.ref ? refUrl(finding.ref) : null;
  return (
    <li className="flex flex-col gap-1 rounded-md border border-border p-3" data-kind={finding.kind}>
      <div className="flex items-start justify-between gap-3">
        <p className="min-w-0 flex-1 text-sm">
          <span className="mr-2 inline-flex items-center gap-1 font-medium">
            <Icon className={`h-4 w-4 shrink-0 ${sev.tone}`} aria-hidden="true" />
            {sev.word}
          </span>
          {finding.sentence}
        </p>
        {open ? (
          <Link to={open} className={`shrink-0 text-sm ${LINK}`}>
            Open
          </Link>
        ) : null}
      </div>
      {finding.sample.length > 0 ? (
        <ul className="flex flex-col gap-0.5 text-sm sm:ml-6">
          {finding.sample.map((s, i) => (
            <li key={`${s.label}-${i}`} className="min-w-0 break-words">
              <RefLink r={s.ref}>{s.label}</RefLink>
              {s.detail ? <span className="text-muted-foreground"> — {s.detail}</span> : null}
            </li>
          ))}
        </ul>
      ) : null}
    </li>
  );
}
