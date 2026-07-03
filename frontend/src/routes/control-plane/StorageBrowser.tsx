import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, ChevronRight, Download, File, Folder, FolderOpen } from "lucide-react";

import {
  type Storage,
  type StorageListing,
  browseStorageObjects,
  getStorage,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";

function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export default function StorageBrowser() {
  const { id = "" } = useParams();
  const [prefix, setPrefix] = useState("");

  const conn = useQuery<Storage, ApiError>({
    queryKey: ["control-plane", "storage", id],
    queryFn: () => getStorage(id),
    retry: false,
  });
  const listing = useQuery<StorageListing, ApiError>({
    queryKey: ["cp", "storage-objects", id, prefix],
    queryFn: () => browseStorageObjects(id, prefix),
    retry: false,
  });

  const parts = prefix.split("/").filter(Boolean);
  const folders = listing.data?.folders ?? [];
  const objects = listing.data?.objects ?? [];
  const bucket = conn.data?.bucket ?? "bucket";

  return (
    <div className="space-y-5">
      <Link
        to="/control-plane/storage"
        className="inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" />
        Storage
      </Link>

      <Card className="overflow-hidden">
        <div className="flex items-center gap-3 border-b bg-muted/30 p-5">
          <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
            <FolderOpen className="h-6 w-6" />
          </div>
          <div className="min-w-0">
            <h1 className="truncate text-2xl font-semibold leading-tight">
              {conn.data?.name ?? "Storage"}
            </h1>
            <p className="font-mono text-xs text-muted-foreground">
              {bucket}
              {conn.data?.endpoint ? ` · ${conn.data.endpoint}` : ""}
            </p>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-1 border-b px-5 py-3 text-sm">
          <button
            type="button"
            className="text-primary hover:underline"
            onClick={() => setPrefix("")}
          >
            {bucket}
          </button>
          {parts.map((part, i) => (
            <span key={i} className="flex items-center gap-1">
              <ChevronRight className="h-3.5 w-3.5 text-muted-foreground" />
              <button
                type="button"
                className="text-primary hover:underline"
                onClick={() => setPrefix(parts.slice(0, i + 1).join("/") + "/")}
              >
                {part}
              </button>
            </span>
          ))}
        </div>

        <CardContent className="p-0">
          {listing.isLoading ? (
            <p className="py-14 text-center text-sm text-muted-foreground">Loading…</p>
          ) : listing.isError ? (
            <p className="py-14 text-center text-sm text-destructive">
              {(listing.error as ApiError).message}
            </p>
          ) : folders.length === 0 && objects.length === 0 ? (
            <p className="py-14 text-center text-sm text-muted-foreground">This folder is empty.</p>
          ) : (
            <div>
              {folders.map((f) => (
                <button
                  key={f}
                  type="button"
                  onClick={() => setPrefix(f)}
                  className="flex w-full items-center gap-2 border-b px-5 py-2.5 text-left text-sm transition-colors hover:bg-muted"
                >
                  <Folder className="h-4 w-4 shrink-0 text-amber-500" />
                  <span className="truncate">{f.slice(prefix.length).replace(/\/$/, "")}</span>
                  <Badge variant="outline" className="ml-auto text-[10px] uppercase">
                    folder
                  </Badge>
                </button>
              ))}
              {objects.map((o) => (
                <div
                  key={o.key}
                  className="flex items-center gap-2 border-b px-5 py-2.5 text-sm last:border-b-0"
                >
                  <File className="h-4 w-4 shrink-0 text-muted-foreground" />
                  <span className="flex-1 truncate font-mono text-xs">{o.key.slice(prefix.length)}</span>
                  <span className="shrink-0 text-xs text-muted-foreground">{fmtSize(o.size)}</span>
                  <a
                    href={`/api/control-plane/storage/${id}/download?key=${encodeURIComponent(o.key)}`}
                    download
                    className="shrink-0 rounded p-1 text-muted-foreground transition-colors hover:text-primary"
                    aria-label={`Download ${o.key}`}
                    title="Download"
                  >
                    <Download className="h-4 w-4" />
                  </a>
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
