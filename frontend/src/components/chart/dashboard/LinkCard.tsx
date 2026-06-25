import type React from "react";

const SAFE_URL = /^https?:\/\//i;

/** A dashboard link card: a clickable tile to an external URL. In edit mode it does not
 *  navigate (the click bubbles to the card wrapper, which opens the card's settings). */
export function LinkCard({ url, label, editing }: { url?: string; label?: string; editing?: boolean }) {
  const href = (url ?? "").trim();
  if (!SAFE_URL.test(href)) {
    return <div className="flex h-full items-center justify-center p-2 text-sm text-muted-foreground">No URL set</div>;
  }
  return (
    <a
      href={href}
      {...(editing ? { onClick: (e: React.MouseEvent) => e.preventDefault() } : { target: "_blank", rel: "noreferrer" })}
      className="flex h-full items-center gap-2 p-2 text-sm text-primary underline"
    >
      {label?.trim() || href}
    </a>
  );
}
