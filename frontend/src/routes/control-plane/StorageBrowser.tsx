import { Navigate, useParams, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { ChevronRight, Download, File, Folder } from "lucide-react";

import {
  type Storage,
  type StorageListing,
  browseStorageObjects,
  getStorage,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { Badge } from "@/components/ui/badge";
import { ListSkeleton } from "@/components/explore/ListSkeleton";

function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

/**
 * Object browser for a storage connection — breadcrumb + folder/file listing,
 * with folder navigation via the `?prefix=` param (a history PUSH so Back walks
 * up one folder). Rendered inside the storage detail's Browse tab.
 */
export function StorageObjectBrowser({ storageId }: { storageId: string }) {
  const [sp, setSp] = useSearchParams();
  const prefix = sp.get("prefix") ?? "";
  const setPrefix = (next: string) =>
    setSp((prev) => {
      const n = new URLSearchParams(prev);
      if (next) n.set("prefix", next);
      else n.delete("prefix");
      return n;
    });

  const conn = useQuery<Storage, ApiError>({
    queryKey: ["control-plane", "storage", storageId],
    queryFn: () => getStorage(storageId),
    retry: false,
  });
  const listing = useQuery<StorageListing, ApiError>({
    queryKey: ["cp", "storage-objects", storageId, prefix],
    queryFn: () => browseStorageObjects(storageId, prefix),
    retry: false,
  });

  const parts = prefix.split("/").filter(Boolean);
  const folders = listing.data?.folders ?? [];
  const objects = listing.data?.objects ?? [];
  const bucket = conn.data?.bucket ?? "bucket";
  // The connection's Key prefix is its root: keys below are relative to it.
  const root = (listing.data?.root ?? "").replace(/\/$/, "");
  const hidden = listing.data?.hiddenSystemFolders ?? [];

  return (
    <div className="overflow-hidden rounded-md border">
      <div className="flex flex-wrap items-center gap-1 border-b bg-muted/30 px-4 py-2.5 text-sm">
        <button type="button" className="font-mono text-primary hover:underline" onClick={() => setPrefix("")}>
          {root ? `${bucket}/${root}` : bucket}
        </button>
        {parts.map((part, i) => (
          <span key={i} className="flex items-center gap-1">
            <ChevronRight className="h-3.5 w-3.5 text-muted-foreground" />
            <button
              type="button"
              className="font-mono text-primary hover:underline"
              onClick={() => setPrefix(parts.slice(0, i + 1).join("/") + "/")}
            >
              {part}
            </button>
          </span>
        ))}
      </div>

      {listing.isLoading ? (
        <div className="p-4">
          <ListSkeleton rows={8} cols={2} />
        </div>
      ) : listing.isError ? (
        <p className="py-14 text-center text-sm text-destructive">
          {(listing.error as ApiError).message}
        </p>
      ) : folders.length === 0 && objects.length === 0 && hidden.length === 0 ? (
        <p className="py-14 text-center text-sm text-muted-foreground">This folder is empty.</p>
      ) : (
        <div>
          {folders.map((f) => (
            <button
              key={f}
              type="button"
              onClick={() => setPrefix(f)}
              className="flex w-full items-center gap-2 border-b px-4 py-2.5 text-left text-sm transition-colors last:border-b-0 hover:bg-muted"
            >
              <Folder className="h-4 w-4 shrink-0 text-highlight" />
              <span className="truncate">{f.slice(prefix.length).replace(/\/$/, "")}</span>
              <Badge variant="outline" className="ml-auto text-xs uppercase">
                folder
              </Badge>
            </button>
          ))}
          {objects.map((o) => (
            <div
              key={o.key}
              className="flex items-center gap-2 border-b px-4 py-2.5 text-sm last:border-b-0"
            >
              <File className="h-4 w-4 shrink-0 text-muted-foreground" />
              <span className="flex-1 truncate font-mono text-xs">{o.key.slice(prefix.length)}</span>
              <span className="shrink-0 text-xs tabular-nums text-muted-foreground">{fmtSize(o.size)}</span>
              <a
                href={`/api/control-plane/storage/${storageId}/download?key=${encodeURIComponent(o.key)}`}
                download
                className="shrink-0 rounded p-1 text-muted-foreground transition-colors hover:text-primary"
                aria-label={`Download ${o.key}`}
                title="Download"
              >
                <Download className="h-4 w-4" />
              </a>
            </div>
          ))}
          {hidden.length > 0 && (
            <p className="px-4 py-2.5 text-xs text-muted-foreground" data-testid="storage-hidden-system">
              {hidden.map((h) => h.slice(prefix.length).replace(/\/$/, "")).join(", ")}{" "}
              {hidden.length === 1 ? "is" : "are"} not shown: egret stores its users&apos; SQL Lab
              results, Data App sources and workflow files there. Only a platform admin can open{" "}
              {hidden.length === 1 ? "it" : "them"}.
            </p>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * Legacy `/control-plane/storage/:id/browse` — browsing now lives on the storage
 * detail page, so redirect (preserving any `?prefix=` deep-link).
 */
export default function StorageBrowserRedirect() {
  const { id = "" } = useParams();
  const [sp] = useSearchParams();
  const prefix = sp.get("prefix");
  const suffix = prefix ? `&prefix=${encodeURIComponent(prefix)}` : "";
  return <Navigate to={`/control-plane/storage/${id}?tab=browse${suffix}`} replace />;
}
