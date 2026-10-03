import { useEffect, useRef, useState } from "react";
import { ArrowLeft, Loader2, RefreshCw, Rocket } from "lucide-react";
import { Link, useParams } from "react-router-dom";
import { toast } from "sonner";

import { requestAppTicket, useApp, usePublishApp } from "@/api/apps";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { describeFailure } from "@/lib/apps/failures";

/**
 * The editor for one app: egret's header over an iframe that shows marimo from the APPS origin.
 *
 * The iframe is a different origin from egret on purpose (spec §6.1): an app's author controls the
 * JavaScript that runs in it, so it must not share egret's origin, its cookies or its storage. The
 * `sandbox` keeps `allow-same-origin` — which is relative to the apps origin, not egret's — because
 * marimo needs its own cookies and storage, and leaves out `allow-top-navigation` so the page
 * cannot navigate egret away.
 *
 * The URL is a one-time ticket that lives for seconds: it is requested once per load, written to
 * the iframe once, and never cached. "Reload editor" asks for a new one.
 */
const SANDBOX = "allow-scripts allow-same-origin allow-forms allow-downloads";

type Load =
  | { state: "ticketing" }
  | { state: "ready"; url: string }
  | { state: "failed"; title: string; message: string };

export default function AppEditPage() {
  const { workspace, id } = useParams();
  const ws = workspace ?? "";
  const appId = id ?? "";
  const { data: app } = useApp(ws, appId, { refetchMs: 5000 });
  const publish = usePublishApp(ws, appId);

  const [attempt, setAttempt] = useState(0);
  const [load, setLoad] = useState<Load>({ state: "ticketing" });
  const [frameLoaded, setFrameLoaded] = useState(false);
  // One ticket per (app, attempt). React runs this effect twice in development (mount, cleanup,
  // mount), and a ticket is spent by its first use, so the second run must not ask again — but it
  // also must not be the one that throws the first run's answer away. So the answer is accepted
  // while its key is still the CURRENT one (`requested`), not while "this run of the effect" is.
  const requested = useRef<string | null>(null);

  useEffect(() => {
    const key = `${ws}/${appId}/${attempt}`;
    if (requested.current === key) return;
    requested.current = key;
    setLoad({ state: "ticketing" });
    setFrameLoaded(false);
    requestAppTicket(ws, appId, "edit")
      .then((url) => {
        if (requested.current === key) setLoad({ state: "ready", url });
      })
      .catch((e) => {
        if (requested.current === key) setLoad({ state: "failed", ...describeFailure(e) });
      });
  }, [ws, appId, attempt]);

  async function doPublish() {
    try {
      const v = await publish.mutateAsync({ expectedRevision: app?.draft_revision });
      toast.success(`Published version ${v.version_no}`);
    } catch (e) {
      const status = (e as { status?: number }).status;
      toast.error(
        status === 409
          ? "The draft changed while you were publishing. Save in the editor, then publish again."
          : String((e as Error)?.message ?? e),
      );
    }
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-3 border-b px-4 py-2">
        <Button size="icon" variant="ghost" asChild aria-label="Back to apps">
          <Link to={`/explore/${ws}/apps`}>
            <ArrowLeft className="h-4 w-4" />
          </Link>
        </Button>
        <h1 className="truncate text-base font-semibold">{app?.name ?? "App"}</h1>
        {app &&
          (app.latest_version_no == null ? (
            <Badge variant="outline">Draft only</Badge>
          ) : (
            <Badge variant="secondary">Published v{app.latest_version_no}</Badge>
          ))}
        {app && <span className="text-xs text-muted-foreground">Draft revision {app.draft_revision}</span>}
        <div className="ml-auto flex items-center gap-2">
          <span className="hidden text-xs text-muted-foreground md:inline">
            Save in the editor (Ctrl/⌘+S) before publishing.
          </span>
          <Button size="sm" variant="outline" onClick={() => setAttempt((n) => n + 1)}>
            <RefreshCw className="h-4 w-4" />
            Reload editor
          </Button>
          <Button size="sm" onClick={doPublish} disabled={!app || publish.isPending}>
            {publish.isPending ? <Loader2 className="h-4 w-4 animate-spin" /> : <Rocket className="h-4 w-4" />}
            {app ? `Publish draft rev ${app.draft_revision}` : "Publish"}
          </Button>
        </div>
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
            // A new key per attempt makes "Reload editor" a fresh iframe rather than a navigation
            // of the old one (whose ticket is already spent).
            key={`${appId}-${attempt}`}
            title={`${app?.name ?? "App"} editor`}
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
            Starting your editor…
            <span className="text-xs">The first open after a while can take up to a minute.</span>
          </div>
        )}
      </div>
    </div>
  );
}
