import { useState } from "react";
import { Copy, KeyRound } from "lucide-react";
import type { CreatedCredential } from "@/api/serviceAccounts";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

/**
 * The one-time secret of a new or rotated credential. The caller holds the
 * `CreatedCredential` in component state only and drops it in `onClose`: the
 * server keeps a hash, so the value can never be fetched again.
 */
export function SecretReveal({ created, onClose }: { created: CreatedCredential | null; onClose: () => void }) {
  const [copied, setCopied] = useState(false);
  const [copyFailed, setCopyFailed] = useState(false);

  const close = () => {
    setCopied(false);
    setCopyFailed(false);
    onClose();
  };
  const copy = async (value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setCopyFailed(false);
    } catch {
      setCopied(false);
      setCopyFailed(true);
    }
  };

  return (
    <Dialog open={created !== null} onOpenChange={(open) => (open ? undefined : close())}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Credential secret</DialogTitle>
          <DialogDescription>Copy the client secret now and store it somewhere safe.</DialogDescription>
        </DialogHeader>
        {created ? (
          <div className="space-y-4">
            <Alert role="note">
              <KeyRound aria-hidden className="h-4 w-4" />
              <AlertTitle>This secret won&apos;t be shown again</AlertTitle>
              <AlertDescription>
                egret stores only a hash of it. If you lose it, rotate the credential to get a new one.
              </AlertDescription>
            </Alert>
            <div className="space-y-1.5">
              <Label htmlFor="credential-client-id">Client ID</Label>
              <Input id="credential-client-id" readOnly value={created.client_id} className="font-mono" />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="credential-client-secret">Client secret</Label>
              <div className="flex gap-2">
                <Input id="credential-client-secret" readOnly value={created.client_secret} className="font-mono" />
                <Button type="button" variant="outline" onClick={() => void copy(created.client_secret)}>
                  <Copy aria-hidden className="mr-2 h-4 w-4" />
                  Copy secret
                </Button>
              </div>
              <p role="status" className="min-h-5 text-xs text-muted-foreground">
                {copied ? "Copied to the clipboard." : copyFailed ? "Could not copy. Select the secret and copy it by hand." : ""}
              </p>
            </div>
          </div>
        ) : null}
        <DialogFooter>
          <Button type="button" onClick={close}>
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
