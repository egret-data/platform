import { useState } from "react";
import { useParams } from "react-router-dom";
import { HardDrive, Loader2, TriangleAlert } from "lucide-react";

import {
  type UpdateStorageRequest,
  type WorkspaceStorageInfo,
  useUpdateStorage,
  useWorkspaceStorage,
} from "@/api/catalog";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";

export default function StoragePage() {
  const ws = useParams().workspace!;
  const storage = useWorkspaceStorage(ws);

  if (storage.isLoading) {
    return (
      <section>
        <h1 className="text-xl font-semibold">Storage</h1>
        <SectionHeader />
        <div className="mt-4 space-y-2">
          <Skeleton className="h-5 w-1/3" />
          <Skeleton className="h-5 w-1/2" />
          <Skeleton className="h-5 w-2/5" />
          <Skeleton className="h-5 w-1/4" />
        </div>
      </section>
    );
  }

  if (storage.isError) {
    const status = (storage.error as ApiError | null)?.status;

    if (status === 404) {
      return (
        <section>
          <h1 className="text-xl font-semibold">Storage</h1>
          <SectionHeader />
          <div className="mt-4 rounded-lg border border-border bg-muted/40 p-5 text-sm text-muted-foreground">
            <p>This workspace uses the platform default storage.</p>
            <p className="mt-2 text-xs text-muted-foreground/70">
              Customizing storage for a default-backed workspace isn&apos;t supported here yet
              &mdash; set it at workspace creation.
            </p>
          </div>
        </section>
      );
    }

    if (status === 403) {
      return (
        <section>
          <h1 className="text-xl font-semibold">Storage</h1>
          <SectionHeader />
          <Alert className="mt-4">
            <HardDrive className="h-4 w-4" />
            <AlertTitle>Insufficient permissions</AlertTitle>
            <AlertDescription>
              Requires workspace-admin to view storage configuration.
            </AlertDescription>
          </Alert>
        </section>
      );
    }

    return (
      <section>
        <h1 className="text-xl font-semibold">Storage</h1>
        <SectionHeader />
        <Alert variant="destructive" className="mt-4">
          <AlertTitle>Failed to load storage configuration</AlertTitle>
          <AlertDescription>Something went wrong. Please try again.</AlertDescription>
        </Alert>
      </section>
    );
  }

  const info = storage.data!;

  return (
    <section>
      <h1 className="text-xl font-semibold">Storage</h1>
      <SectionHeader />
      <StorageEditForm ws={ws} info={info} />
    </section>
  );
}

function SectionHeader() {
  return (
    <p className="mt-1 text-sm text-muted-foreground">
      Manage this workspace&apos;s catalog storage credentials and connection settings.
    </p>
  );
}

function StorageEditForm({ ws, info }: { ws: string; info: WorkspaceStorageInfo }) {
  const updateMutation = useUpdateStorage(ws);

  const [endpoint, setEndpoint] = useState(info.endpoint ?? "");
  const [region, setRegion] = useState(info.region);
  const [accessKey, setAccessKey] = useState(info.access_key);
  const [secretKey, setSecretKey] = useState("");
  const [pathStyle, setPathStyle] = useState(false);

  const [successMsg, setSuccessMsg] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setSuccessMsg(null);
    setErrorMsg(null);

    const req: UpdateStorageRequest = {
      provider: info.provider,
      bucket: info.bucket,
      key_prefix: info.key_prefix,
      endpoint: endpoint.trim().length > 0 ? endpoint.trim() : null,
      region: region.trim(),
      access_key: accessKey.trim(),
      secret_key: secretKey.length > 0 ? secretKey : null,
      path_style: pathStyle,
    };

    try {
      await updateMutation.mutateAsync(req);
      setSuccessMsg("Storage updated.");
      setSecretKey(""); // clear secret field after save
    } catch (err) {
      setErrorMsg((err as ApiError).message ?? "Failed to update storage.");
    }
  }

  const keyPrefixDisplay =
    info.key_prefix && info.key_prefix.length > 0
      ? info.key_prefix
      : `workspaces/${ws}/warehouse`;

  return (
    <form onSubmit={handleSubmit} className="mt-4 space-y-5">
      {/* Re-point warning */}
      <Alert className="border-amber-500/40 bg-amber-50/60 text-amber-900 dark:bg-amber-900/20 dark:text-amber-200">
        <TriangleAlert className="h-4 w-4 text-amber-500" />
        <AlertTitle>Changing credentials re-points this catalog warehouse</AlertTitle>
        <AlertDescription>
          Updating the endpoint or credentials re-points this workspace&apos;s catalog warehouse.
          The bucket and location are fixed after provisioning; existing data is not migrated.
        </AlertDescription>
      </Alert>

      <Card>
        <CardContent className="pt-6 space-y-5">
          {/* Fixed / read-only fields */}
          <fieldset className="space-y-4">
            <legend className="text-xs font-semibold uppercase tracking-wide text-muted-foreground mb-2">
              Fixed (set at provisioning)
            </legend>

            <div className="space-y-1.5">
              <Label htmlFor="storage-provider" className="text-muted-foreground">
                Provider
              </Label>
              <Input
                id="storage-provider"
                value={info.provider}
                disabled
                aria-label="Provider (fixed)"
              />
              <p className="text-xs text-muted-foreground">Fixed after provisioning.</p>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="storage-bucket" className="text-muted-foreground">
                Bucket
              </Label>
              <Input
                id="storage-bucket"
                value={info.bucket}
                disabled
                aria-label="Bucket (fixed)"
              />
              <p className="text-xs text-muted-foreground">Fixed after provisioning.</p>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="storage-key-prefix" className="text-muted-foreground">
                Key prefix
              </Label>
              <Input
                id="storage-key-prefix"
                value={keyPrefixDisplay}
                disabled
                aria-label="Key prefix (fixed)"
              />
              <p className="text-xs text-muted-foreground">Fixed after provisioning.</p>
            </div>
          </fieldset>

          {/* Editable fields */}
          <fieldset className="space-y-4 border-t border-border pt-5">
            <legend className="text-xs font-semibold uppercase tracking-wide text-muted-foreground mb-2">
              Credentials &amp; connection (editable)
            </legend>

            <div className="space-y-1.5">
              <Label htmlFor="storage-endpoint">Endpoint</Label>
              <Input
                id="storage-endpoint"
                value={endpoint}
                onChange={(e) => setEndpoint(e.target.value)}
                placeholder="Leave blank for AWS default (e.g. https://s3.example.com)"
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="storage-region">Region</Label>
              <Input
                id="storage-region"
                value={region}
                onChange={(e) => setRegion(e.target.value)}
                placeholder="e.g. us-east-1"
                required
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="storage-access-key">Access key</Label>
              <Input
                id="storage-access-key"
                value={accessKey}
                onChange={(e) => setAccessKey(e.target.value)}
                placeholder="Access key ID"
                required
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="storage-secret-key">Secret key</Label>
              <Input
                id="storage-secret-key"
                type="password"
                value={secretKey}
                onChange={(e) => setSecretKey(e.target.value)}
                placeholder="Leave blank to keep current"
                autoComplete="new-password"
              />
              {info.has_secret && (
                <p className="text-xs text-muted-foreground">A secret is currently configured.</p>
              )}
            </div>

            <div className="flex items-center gap-3">
              <input
                type="checkbox"
                id="storage-path-style"
                checked={pathStyle}
                onChange={(e) => setPathStyle(e.target.checked)}
                className="h-4 w-4 rounded border-border accent-primary cursor-pointer"
              />
              <Label htmlFor="storage-path-style">
                Force path-style URL (s3.region.amazonaws.com/bucket)
              </Label>
            </div>
          </fieldset>
        </CardContent>
      </Card>

      {successMsg && (
        <Alert>
          <AlertTitle>Success</AlertTitle>
          <AlertDescription>{successMsg}</AlertDescription>
        </Alert>
      )}

      {errorMsg && (
        <Alert variant="destructive">
          <AlertTitle>Update failed</AlertTitle>
          <AlertDescription>{errorMsg}</AlertDescription>
        </Alert>
      )}

      <div className="flex justify-end">
        <Button type="submit" disabled={updateMutation.isPending}>
          {updateMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
          Save
        </Button>
      </div>
    </form>
  );
}
