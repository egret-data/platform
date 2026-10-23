import { useEffect, useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import { useParams } from "react-router-dom";

import { requestAppTicket, useApp } from "@/api/apps";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { describeFailure } from "@/lib/apps/failures";
import { PageTitle } from "@/components/layout/PageTitle";
import { BackLink } from "@/components/layout/DetailHeader";

/**
 * A published app, as its viewer: egret's thin header over an iframe showing marimo from the
 * APPS origin. The ticket-lifecycle logic (one ticket per load, never cached or reused, ignore a
 * stale answer) is identical to `AppEditPage`'s — only the chrome and the mode differ. No file is
 * named: "run" with no version id always opens the app's latest published version.
 */
const SANDBOX = "allow-scripts allow-same-origin allow-forms allow-downloads";

type Load =
  | { state: "ticketing" }
  | { state: "ready"; url: string }
  | { state: "failed"; title: string; message: string };

export default function AppViewPage() {
  const { workspace, id } = useParams();
  const ws = workspace ?? "";
  const appId = id ?? "";
  const { data: app } = useApp(ws, appId);

  const [load, setLoad] = useState<Load>({ state: "ticketing" });
  const [frameLoaded, setFrameLoaded] = useState(false);
  // Same race guard as AppEditPage: React's double-mount in development must not spend a second
  // ticket, and a late answer from a stale run must not override the current one.
  const requested = useRef<string | null>(null);

  useEffect(() => {
    const key = `${ws}/${appId}`;
    if (requested.current === key) return;
    requested.current = key;
    setLoad({ state: "ticketing" });
    setFrameLoaded(false);
    requestAppTicket(ws, appId, "run")
      .then((url) => {
        if (requested.current === key) setLoad({ state: "ready", url });
      })
      .catch((e) => {
        if (requested.current === key) setLoad({ state: "failed", ...describeFailure(e, "run") });
      });
  }, [ws, appId]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-3 border-b px-4 py-2">
        {/* A tool header (FE-PAGE-7): the back link and the compact title. */}
        <BackLink to={`/explore/${ws}/apps`}>Apps</BackLink>
        <PageTitle size="compact" className="truncate">{app?.name ?? "App"}</PageTitle>
      </div>

      <div className="relative min-h-0 flex-1">
        {load.state === "failed" && (
          <div className="p-4">
            <Alert variant="destructive">
              <AlertTitle>{load.title}</AlertTitle>
              <AlertDescription>{load.message}</AlertDescription>
            </Alert>
          </div>
        )}
        {load.state === "ready" && (
          <iframe
            title={app?.name ?? "App"}
            src={load.url}
            sandbox={SANDBOX}
            // The ticket is in the URL; the apps origin should never see egret's address as a Referer.
            referrerPolicy="no-referrer"
            allow="clipboard-write"
            className="h-full w-full border-0"
            onLoad={() => setFrameLoaded(true)}
          />
        )}
        {load.state !== "failed" && !frameLoaded && (
          <div
            role="status"
            className="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-background text-sm text-muted-foreground"
          >
            <Loader2 className="h-5 w-5 animate-spin" />
            Starting the app…
            <span className="text-xs">The first open after a while can take up to a minute.</span>
          </div>
        )}
      </div>
    </div>
  );
}
