import { useParams } from "react-router-dom";
import { HardDrive } from "lucide-react";

import { type WorkspaceStorageInfo, useWorkspaceStorage } from "@/api/catalog";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
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
            This workspace uses the platform default storage.
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
      <Card className="mt-4">
        <CardContent className="pt-6">
          <StorageInfoGrid ws={ws} info={info} />
        </CardContent>
      </Card>
      <p className="mt-3 text-xs text-muted-foreground">
        Storage is set when the workspace is created; changing it isn&apos;t supported yet.
      </p>
    </section>
  );
}

function SectionHeader() {
  return (
    <p className="mt-1 text-sm text-muted-foreground">
      Read-only view of this workspace&apos;s catalog storage configuration.
    </p>
  );
}

function StorageInfoGrid({ ws, info }: { ws: string; info: WorkspaceStorageInfo }) {
  const keyPrefix =
    info.key_prefix && info.key_prefix.length > 0
      ? info.key_prefix
      : null;

  const endpoint =
    info.endpoint && info.endpoint.length > 0
      ? info.endpoint
      : null;

  const rows: { label: string; value: React.ReactNode }[] = [
    { label: "Provider", value: <span className="font-mono">{info.provider}</span> },
    { label: "Bucket", value: <span className="font-mono">{info.bucket}</span> },
    {
      label: "Endpoint",
      value: endpoint ? (
        <span className="font-mono">{endpoint}</span>
      ) : (
        <span className="text-muted-foreground">AWS default</span>
      ),
    },
    { label: "Region", value: <span className="font-mono">{info.region}</span> },
    {
      label: "Key prefix",
      value: keyPrefix ? (
        <span className="font-mono">{keyPrefix}</span>
      ) : (
        <span className="text-muted-foreground">
          default (workspaces/{ws}/warehouse)
        </span>
      ),
    },
    { label: "Access key", value: <span className="font-mono">{info.access_key}</span> },
    {
      label: "Secret",
      value: info.has_secret ? (
        <Badge variant="secondary">Configured</Badge>
      ) : (
        <Badge variant="outline">Not set</Badge>
      ),
    },
  ];

  return (
    <dl className="divide-y divide-border text-sm">
      {rows.map(({ label, value }) => (
        <div key={label} className="flex items-center gap-4 py-2.5">
          <dt className="w-32 shrink-0 font-medium text-muted-foreground">{label}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}
