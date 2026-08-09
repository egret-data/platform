import { Lock } from "lucide-react";
import { IFRAME_REFERRER_POLICY, IFRAME_SANDBOX, resolveIframeSrc } from "./iframe-url";

/** A dashboard iframe card: third-party content embedded inside an authenticated app.
 *
 *  Every render path goes through `resolveIframeSrc`, which fails closed — a setting this
 *  component cannot prove safe renders a stated reason, never an empty box and never an
 *  <iframe> with an unchecked src. The card holds no policy of its own; if you find yourself
 *  adding a URL check here, it belongs in `iframe-url.ts` where it is testable without React.
 *
 *  No card header: like the other virtual card types (text/heading/link) an iframe card carries
 *  no title, no Edit-question pencil and no description popover — `DashboardCardHeader` hangs off
 *  a saved chart and an iframe card has none. The one piece of W5 chrome it DOES get is the
 *  background toggle, which lives on the tile wrapper via `showsCardBackground` and needs nothing
 *  from this component. */
export function IFrameCard({ iframe, editing }: { iframe?: string; editing?: boolean }) {
  const resolved = resolveIframeSrc(iframe);

  if (!resolved.ok) {
    if (resolved.reason === "empty") {
      return (
        <div className="flex h-full items-center justify-center p-2 text-sm text-muted-foreground">
          No embed set
        </div>
      );
    }
    // The host is named only while editing, matching Metabase's split between
    // ForbiddenDomainError and GenericError (IFrameViz.tsx:134-139). A shared dashboard's viewer
    // cannot fix the card, so naming the host would leak it for nothing.
    const namesHost = editing && resolved.reason === "forbidden-host" && !!resolved.host;
    return (
      <div className="flex h-full flex-col items-center justify-center gap-1 p-3 text-center">
        <Lock className="h-4 w-4 text-muted-foreground" aria-hidden />
        <p className="text-sm text-foreground">
          {namesHost
            ? `${resolved.host} can't be embedded in an iframe card.`
            : "There was a problem rendering this content."}
        </p>
        {editing && (
          <p className="text-xs text-muted-foreground">
            Only https pages on egret's allowed hosts can be embedded.
          </p>
        )}
      </div>
    );
  }

  return (
    <iframe
      title="Embedded content"
      // A live iframe swallows pointer events, so while editing it would eat the click that
      // opens the card's settings panel and the drag that moves the card.
      className={`h-full w-full border-0 ${editing ? "pointer-events-none" : ""}`}
      sandbox={IFRAME_SANDBOX}
      referrerPolicy={IFRAME_REFERRER_POLICY}
      {...(resolved.attrs.allow ? { allow: resolved.attrs.allow } : {})}
      allowFullScreen={resolved.attrs.allowFullScreen}
      src={resolved.attrs.src}
    />
  );
}
