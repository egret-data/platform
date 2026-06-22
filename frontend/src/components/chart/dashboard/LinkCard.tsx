const SAFE_URL = /^https?:\/\//i;

/** A dashboard link card: a clickable tile to an external URL. */
export function LinkCard({ url, label }: { url?: string; label?: string }) {
  const href = (url ?? "").trim();
  if (!SAFE_URL.test(href)) {
    return <div className="flex h-full items-center justify-center p-2 text-sm text-muted-foreground">No URL set</div>;
  }
  return (
    <a href={href} target="_blank" rel="noreferrer" className="flex h-full items-center gap-2 p-2 text-sm text-primary underline">
      {label?.trim() || href}
    </a>
  );
}
