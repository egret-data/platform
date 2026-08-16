import { useState } from "react";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ApiError } from "@/api/client";
import { dataApiGetOpenapi, useDataApiListServiceAccounts } from "@/api/generated/data-api/data-api";

/**
 * Spec §5.6: a button on the Endpoints list opens this to pick a service
 * account, then downloads the generated OpenAPI 3.0.3 document describing
 * every LIVE endpoint that account may invoke. On endpoint detail (Task 4),
 * the Response section opens the SAME dialog with `endpointId` pinned, which
 * narrows the same document to one path — one component, so the two surfaces
 * cannot drift about which account they asked for or how they render a
 * failure.
 *
 * `GET /api/ws/{ws}/data-api/openapi?service_account_id=…[&endpoint_id=…]` is
 * gated on `data_api_endpoint:manage` (spec §5.1) — a viewer with only
 * `:read` can see this list and still get a 403 here, so that failure is
 * rendered as a permission message rather than swallowed or left as a dead
 * button. A 400 (more than 200 live endpoints, undocumented for the
 * workspace-wide document) carries its own remedy — `?endpoint_id=` — in the
 * body's `error` string, so that message is shown verbatim.
 */
export function DownloadOpenApiDialog({ ws, open, onOpenChange, endpointId }: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Pin the document to one endpoint (Task 4's detail-page call). Omitted on
   *  the list page, which asks for the aggregate document. */
  endpointId?: string;
}) {
  const { data: envelope, isLoading: accountsLoading } = useDataApiListServiceAccounts(ws);
  const accounts = envelope?.status === 200 ? envelope.data : [];
  const [accountId, setAccountId] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  function reset() {
    setAccountId("");
    setError(null);
    setPending(false);
  }

  async function download() {
    if (!accountId) return;
    setPending(true);
    setError(null);
    try {
      const res = await dataApiGetOpenapi(ws, { service_account_id: accountId, endpoint_id: endpointId });
      const blob = new Blob([JSON.stringify(res.data, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = endpointId ? `openapi-${endpointId}.json` : "openapi.json";
      a.click();
      URL.revokeObjectURL(url);
      onOpenChange(false);
    } catch (e) {
      if (e instanceof ApiError && e.status === 403) {
        // The generic "forbidden" body carries no remedy; :manage-vs-:read is
        // the whole reason this is worth naming rather than showing raw text.
        setError("You need the data_api_endpoint:manage permission to download this document.");
      } else if (e instanceof ApiError) {
        setError(e.message);
      } else {
        setError(String(e));
      }
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={(o) => { if (!o) reset(); onOpenChange(o); }}>
      <DialogContent>
        <DialogHeader><DialogTitle>Download OpenAPI</DialogTitle></DialogHeader>
        <p className="text-sm text-muted-foreground">
          {endpointId
            ? "Choose the service account this document describes invoke rights for."
            : "Choose a service account. The document lists every live endpoint that account may invoke."}
        </p>
        <div className="space-y-1.5">
          <Label htmlFor="oa-account">Service account</Label>
          <Select value={accountId} onValueChange={setAccountId}>
            <SelectTrigger id="oa-account" aria-label="Service account">
              <SelectValue placeholder="Choose a service account" />
            </SelectTrigger>
            <SelectContent>
              {accounts.map((a) => (
                <SelectItem key={a.id} value={a.id}>{a.name}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          {!accountsLoading && accounts.length === 0 && (
            <p className="text-xs text-muted-foreground">No service accounts yet — create one under Clients.</p>
          )}
        </div>
        {error && (
          <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={download} disabled={!accountId || pending}>
            {pending ? "Downloading…" : "Download"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
