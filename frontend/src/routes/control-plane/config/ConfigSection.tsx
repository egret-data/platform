import { ExternalLink } from "lucide-react";
import type { ReactNode } from "react";

/**
 * The single card wrapper for a group of related configuration fields:
 * a bordered card with a title (plus optional docs link), an optional
 * muted description, and the field children.
 */
export function ConfigSection({
  title,
  description,
  docsUrl,
  children,
}: {
  title: string;
  description?: string;
  docsUrl?: string;
  children: ReactNode;
}) {
  return (
    <section className="rounded-lg border p-4 space-y-4">
      <div className="space-y-1">
        <div className="flex items-center gap-1.5">
          <h3 className="text-sm font-medium">{title}</h3>
          {docsUrl ? (
            <a
              href={docsUrl}
              target="_blank"
              rel="noreferrer"
              aria-label={`${title} documentation`}
              className="inline-flex text-muted-foreground hover:text-foreground"
            >
              <ExternalLink className="size-3.5" />
            </a>
          ) : null}
        </div>
        {description ? (
          <p className="text-xs text-muted-foreground">{description}</p>
        ) : null}
      </div>
      {children}
    </section>
  );
}
