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
  level = 3,
  children,
}: {
  title: string;
  description?: string;
  docsUrl?: string;
  /**
   * Heading level, so the document outline stays sequential.
   *
   * Defaults to 3, which is right where these sit under a page h1 AND an h2
   * (EngineDetail's tabs). A page that puts them directly under its h1 passes
   * 2 — otherwise the outline jumps h1 → h3, which a screen-reader user
   * navigating by heading hears as a missing level.
   */
  level?: 2 | 3;
  children: ReactNode;
}) {
  const Heading = level === 2 ? "h2" : "h3";
  return (
    <section className="rounded-lg border p-4 space-y-4">
      <div className="space-y-1">
        <div className="flex items-center gap-1.5">
          <Heading className="text-sm font-medium">{title}</Heading>
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
