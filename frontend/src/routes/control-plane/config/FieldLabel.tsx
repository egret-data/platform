import { ExternalLink, Info } from "lucide-react";
import { Label } from "@/components/ui/label";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { fieldHelp } from "./fieldHelp";

/**
 * A form label that, when the field has help text, appends an info icon with a
 * tooltip explaining the field plus (when present) a docs link.
 *
 * Help can be supplied inline via the `help`/`docsUrl` props (used for
 * catalog-driven fields such as the tuning `knownProperties`); when they are
 * absent it falls back to the `fieldHelp[fieldKey]` map. This lets ad-hoc keys
 * carry help without an entry in `fieldHelp.ts`.
 *
 * Relies on the app-wide `TooltipProvider` in `AppShell`.
 */
export function FieldLabel({
  fieldKey,
  label,
  htmlFor,
  help: inlineHelp,
  docsUrl: inlineDocsUrl,
}: {
  fieldKey?: string;
  label: string;
  htmlFor?: string;
  help?: string;
  docsUrl?: string;
}) {
  const mapped = fieldKey ? fieldHelp[fieldKey] : undefined;
  const help = inlineHelp
    ? { help: inlineHelp, docsUrl: inlineDocsUrl, unit: undefined }
    : mapped;

  return (
    <div className="flex items-center gap-1.5">
      <Label htmlFor={htmlFor}>{label}</Label>
      {help ? (
        <span className="flex items-center gap-1">
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                aria-label={`Help: ${label}`}
                className="inline-flex text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded"
              >
                <Info className="size-3.5" />
              </button>
            </TooltipTrigger>
            <TooltipContent className="max-w-xs">
              {help.help}
              {help.unit ? (
                <span className="text-primary-foreground/70">
                  {" "}
                  ({help.unit})
                </span>
              ) : null}
            </TooltipContent>
          </Tooltip>
          {help.docsUrl ? (
            <a
              href={help.docsUrl}
              target="_blank"
              rel="noreferrer"
              aria-label={`${label} documentation`}
              className="inline-flex text-muted-foreground hover:text-foreground"
            >
              <ExternalLink className="size-3.5" />
            </a>
          ) : null}
        </span>
      ) : null}
    </div>
  );
}
