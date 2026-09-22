import { useState } from "react";
import { Link } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, CircleSlash, Loader2, RefreshCw, Unlink, XCircle } from "lucide-react";
import { toast } from "sonner";

import {
  useControlPlaneCatalogRegistryDetachCatalog,
  useControlPlaneCatalogRegistryRegistryAttachCatalog,
  useControlPlaneCatalogRegistryRetryCatalogApply,
  useControlPlaneCatalogRegistryTestRegistryCatalog,
} from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import { useControlPlaneEnginesListEngines } from "@/api/generated/control-plane-engines/control-plane-engines";
import type { controlPlaneEnginesListEnginesResponse } from "@/api/generated/control-plane-engines/control-plane-engines";
import type {
  CatalogAttachmentDto,
  CatalogTestResultDto,
  RegistryCatalogDto,
} from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { useAuth } from "@/auth/useAuth";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { catalogKey, catalogsQueryKey } from "../queryKeys";

/**
 * The three `applyStatus` values, each with its own badge and its own meaning.
 *
 * The words come from `CatalogAttachmentDto.applyStatus`'s own JSDoc, which is
 * where they reach the client at all — a response description does not. The
 * three variants are deliberately distinct: one badge with three captions
 * makes `failed` read as calmly as `applied`.
 */
function StatusBadge({ a }: { a: CatalogAttachmentDto }) {
  if (a.applyStatus === "failed")
    return (
      <Badge variant="destructive" title={a.applyError ?? undefined}>
        failed
      </Badge>
    );
  if (a.applyStatus === "pending")
    return (
      <Badge
        variant="outline"
        title="egret has recorded this attachment and has not yet been able to tell the engine. A StarRocks FE takes 5-10 minutes to come up after create; the bootstrap or Retry converges it."
      >
        pending
      </Badge>
    );
  return <Badge variant="secondary">applied</Badge>;
}

type Tone = "ok" | "bad" | "neutral";

/**
 * D4. The server picks the check and tells us which one it ran; this
 * function's whole job is to not launder one into the other.
 *
 * `tcp` proves the port answers and NOTHING about credentials — so its line
 * says so in the same sentence, not in a tooltip a screenshot will not carry.
 * `unsupported` carries `ok: false` deliberately (nothing was measured), and
 * is rendered NEUTRAL rather than red: a `tpch` catalog with no engine
 * attached is not broken, and colouring it as a failure teaches operators to
 * ignore the colour.
 *
 * The server's own `message` is rendered underneath, always and unedited. It
 * is not decoration: it carries the prefix explaining why no query ran ("No
 * Trino engine has this catalog applied yet (`sr-eu` is `failed`)…"), which
 * exists nowhere else in the response, and on a failed `query` it is the
 * coordinator's own error, which names neither the check nor the catalog.
 * Hence the split: the client owns the sentence that says WHAT was measured,
 * the server owns the sentence that says what came back.
 */
function checkLine(r: CatalogTestResultDto, catalogName: string): { tone: Tone; what: string } {
  // A `switch` over the union rather than an if/else chain, so that a fourth
  // `check` added to the schema makes this function's declared return type
  // unsatisfiable instead of falling quietly into one of the three branches.
  switch (r.check) {
    case "query":
      return {
        tone: r.ok ? "ok" : "bad",
        what: `Query check — SHOW SCHEMAS FROM ${catalogName}${
          r.engineName ? ` on ${r.engineName}` : ""
        }, which exercises the catalog's credentials.`,
      };
    case "tcp":
      return {
        tone: r.ok ? "ok" : "bad",
        what: `Reachability check — egret opened and closed a TCP connection to ${
          r.target ?? "the host and port in the stored configuration"
        }. It does not verify credentials, TLS or permissions, so it cannot tell this catalog from any other process listening on that port.`,
      };
    case "unsupported":
      return {
        tone: "neutral",
        what: "No test ran: egret had neither an engine that could run the query nor a host it could dial.",
      };
  }
}

const TONE_CLASS: Record<Tone, string> = {
  ok: "border-green-600/40 bg-green-600/5",
  bad: "border-destructive/40 bg-destructive/5",
  neutral: "border-border bg-muted/40",
};

function ToneIcon({ tone }: { tone: Tone }) {
  if (tone === "ok") return <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-green-600" />;
  if (tone === "bad") return <XCircle className="mt-0.5 h-4 w-4 shrink-0 text-destructive" />;
  return <CircleSlash className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />;
}

/**
 * Control Plane → Catalogs → detail → Engines (SP3c D1): attach, detach, apply
 * status, Retry, and one Test.
 */
export function EnginesTab({ catalog }: { catalog: RegistryCatalogDto }) {
  const { me } = useAuth();
  const queryClient = useQueryClient();

  const isAdmin = !!me && me.platform_roles.includes("platform_admin");
  const canUpdate = isAdmin || !!me?.permissions.includes("catalog_registry:update");
  // Reading the engine list, and linking to an engine page, is `engine:read` —
  // which `catalog_registry:read` does not imply. Same reasoning as the
  // warehouse link on this page's header: a link that 403s exactly the persona
  // the section was built for is worse than a plain identifier.
  const canReadEngines = isAdmin || !!me?.permissions.includes("engine:read");

  const [pick, setPick] = useState("");
  const [detachTarget, setDetachTarget] = useState<CatalogAttachmentDto | null>(null);

  /**
   * Both caches that describe this attachment set. The detail query is what
   * this table reads; the list page draws an engine count and the stalled
   * warning from its own query, and would keep showing the old one for
   * `staleTime` otherwise. Both keys are imported, never written out — a
   * hand-written triple matches no cache entry at all, and every test here
   * would still be green.
   */
  function invalidate() {
    void queryClient.invalidateQueries({ queryKey: catalogKey(catalog.id) });
    void queryClient.invalidateQueries({ queryKey: catalogsQueryKey });
  }

  // `<…, ApiError>` for the same reason every other query on this page carries
  // it: the thrown value is an `ApiError` with `.message`, and the generated
  // `TError = ErrorBody` would make `.error` type-check and be `undefined`.
  // `engine:read` is a separate permission, so this can 403 on its own.
  const enginesQ = useControlPlaneEnginesListEngines<
    controlPlaneEnginesListEnginesResponse,
    ApiError
  >({ query: { retry: false, enabled: canUpdate } });

  const attach = useControlPlaneCatalogRegistryRegistryAttachCatalog<ApiError>({
    mutation: {
      onSuccess: (res) => {
        invalidate();
        setPick("");
        // SP3b D1: a StarRocks attach returns 201 even when the FE refused it,
        // with `applyStatus` `failed` and the reason in `applyError`.
        // Reporting "attached" and nothing else would hide the half of the
        // outcome that matters.
        if (res.status === 201 && res.data.applyStatus === "failed")
          toast.error(
            `Attached, but the engine refused it: ${res.data.applyError ?? "no reason given"}`,
          );
        else toast.success("Attached.");
      },
      // `apiFetch` throws an `ApiError`, which carries `.message`. The
      // generated `TError = ErrorBody` would make `.error` type-check and be
      // `undefined` at runtime — an empty toast on every failure.
      onError: (e) => toast.error(e.message || "Could not attach."),
    },
  });

  const detach = useControlPlaneCatalogRegistryDetachCatalog<ApiError>({
    mutation: {
      onSuccess: () => {
        invalidate();
        toast.success("Detached.");
      },
      // Verbatim: the 400 a primary attachment returns names the only way out.
      onError: (e) => toast.error(e.message || "Could not detach."),
    },
  });

  const retry = useControlPlaneCatalogRegistryRetryCatalogApply<ApiError>({
    mutation: {
      onSuccess: () => {
        invalidate();
        toast.success("Re-applied.");
      },
      onError: (e) => toast.error(e.message || "The engine rejected the re-apply."),
    },
  });

  // ONE Test, in the header, not one per row: the route decides which of the
  // three checks is possible from the stored row and the attachments, and
  // duplicating that decision per row in the client is how the two drift.
  const test = useControlPlaneCatalogRegistryTestRegistryCatalog<ApiError>();

  const attached = new Set(catalog.engines.map((a) => a.engineId));
  const candidates = (enginesQ.data?.status === 200 ? enginesQ.data.data : []).filter(
    (e) => !attached.has(e.id),
  );
  const failures = catalog.engines.filter((a) => a.applyStatus === "failed" && a.applyError);

  const result = test.data?.status === 200 ? test.data.data : null;
  const line = result ? checkLine(result, catalog.name) : null;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-sm text-muted-foreground">
          Attaching a catalog makes it a catalog on that engine. Trino applies it to the cluster
          and restarts the coordinator; StarRocks creates it on the FE in place.
        </p>
        <Button
          variant="outline"
          onClick={() => test.mutate({ id: catalog.id })}
          disabled={test.isPending}
        >
          {test.isPending ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <CheckCircle2 className="h-4 w-4" />
          )}
          Test
        </Button>
      </div>

      {(line || test.isError) && (
        <div
          data-testid="catalog-test-result"
          data-tone={line ? line.tone : "bad"}
          className={`flex gap-2 rounded-lg border p-3 text-sm ${
            TONE_CLASS[line ? line.tone : "bad"]
          }`}
        >
          <ToneIcon tone={line ? line.tone : "bad"} />
          {line && result ? (
            <div className="space-y-1">
              <p className="font-medium">{line.what}</p>
              <p className="text-muted-foreground">{result.message}</p>
            </div>
          ) : (
            // The documented 400: the stored configuration does not parse, so
            // there was no target to decide and nothing to test. The message
            // names the field, and it is the one thing the operator can act
            // on, so it is rendered rather than replaced by a generic sentence.
            <p>{test.error?.message ?? "The test could not be run."}</p>
          )}
        </div>
      )}

      {catalog.engines.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          This catalog is not attached to any engine, so nothing can query it yet.
        </p>
      ) : (
        <div className="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Engine</TableHead>
                <TableHead>Type</TableHead>
                <TableHead>Primary</TableHead>
                <TableHead>Status</TableHead>
                <TableHead>Applied</TableHead>
                <TableHead className="text-right">Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {catalog.engines.map((a) => (
                <TableRow key={a.engineId}>
                  <TableCell className="font-medium">
                    {canReadEngines ? (
                      <Link
                        to={`/control-plane/engines/${a.engineId}`}
                        className="hover:underline"
                      >
                        {a.engineName}
                      </Link>
                    ) : (
                      a.engineName
                    )}
                  </TableCell>
                  <TableCell>
                    <Badge variant="outline">{a.engineType}</Badge>
                  </TableCell>
                  <TableCell>
                    {a.isPrimary ? <Badge variant="secondary">primary</Badge> : null}
                  </TableCell>
                  <TableCell>
                    <StatusBadge a={a} />
                  </TableCell>
                  <TableCell className="text-sm text-muted-foreground">
                    {a.appliedAt
                      ? new Date(a.appliedAt).toLocaleString()
                      : `waiting since ${new Date(a.createdAt).toLocaleString()}`}
                  </TableCell>
                  <TableCell className="text-right">
                    {canUpdate && (
                      <div className="flex flex-col items-end gap-1">
                        <div className="flex items-center justify-end gap-2">
                          {a.applyStatus !== "applied" && (
                            <Button
                              variant="outline"
                              size="sm"
                              onClick={() => retry.mutate({ id: a.engineId, cid: catalog.id })}
                              disabled={retry.isPending}
                            >
                              <RefreshCw className="h-3.5 w-3.5" />
                              Retry
                            </Button>
                          )}
                          <Button
                            variant="outline"
                            size="sm"
                            onClick={() => setDetachTarget(a)}
                            disabled={a.isPrimary || detach.isPending}
                          >
                            <Unlink className="h-3.5 w-3.5" />
                            Detach
                          </Button>
                        </div>
                        {a.isPrimary && (
                          // The server is still the refusal point — this
                          // button is a courtesy, not a guarantee — but a
                          // disabled button whose reason is on screen beats a
                          // refusal collected after typing a confirmation word.
                          <span className="text-xs text-muted-foreground">
                            This is {a.engineName}&apos;s primary catalog. Detach it by deleting
                            the engine.
                          </span>
                        )}
                      </div>
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}

      {/* `applyError` in full, below the table. It is the one string that says
          what to fix, and a `title` attribute is not something an operator can
          read at a glance, copy, or capture in a screenshot. */}
      {failures.length > 0 && (
        <Alert variant="destructive" data-testid="catalog-apply-errors">
          <AlertDescription>
            <ul className="space-y-1">
              {failures.map((a) => (
                <li key={a.engineId}>
                  <span className="font-medium">{a.engineName}</span>: {a.applyError}
                </li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      )}

      {canUpdate && (
        <div className="space-y-2 border-t pt-4">
          <Label htmlFor="catalog-attach-engine">Engine to attach</Label>
          <div className="flex flex-wrap items-center gap-2">
            <Select value={pick} onValueChange={setPick}>
              <SelectTrigger id="catalog-attach-engine" className="w-72">
                <SelectValue placeholder="Select an engine" />
              </SelectTrigger>
              <SelectContent>
                {candidates.map((e) => (
                  <SelectItem key={e.id} value={e.id}>
                    {e.name} ({e.type})
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              onClick={() => attach.mutate({ id: catalog.id, engineId: pick })}
              disabled={!pick || attach.isPending}
            >
              {attach.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Attach
            </Button>
          </div>
          {enginesQ.isError ? (
            <p className="text-xs text-muted-foreground">
              {enginesQ.error?.message ?? "Couldn't list the engines to attach to."}
            </p>
          ) : !enginesQ.isLoading && candidates.length === 0 ? (
            <p className="text-xs text-muted-foreground">
              Every engine egret can show you already has this catalog.
            </p>
          ) : null}
        </div>
      )}

      <ConfirmDeleteDialog
        open={detachTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDetachTarget(null);
        }}
        title={
          detachTarget
            ? `Detach «${catalog.name}» from «${detachTarget.engineName}»?`
            : "Detach catalog?"
        }
        description={
          detachTarget
            ? `${detachTarget.engineName} will no longer serve this catalog, and the credential egret minted for the attachment is revoked. The catalog itself is kept, with its configuration and its other attachments.`
            : ""
        }
        // The catalog's own name, not the word "delete": this removes the
        // catalog from ONE engine, and a dialog that asks for "delete" reads
        // like the destructive operation it is not.
        confirmText={catalog.name}
        confirmLabel="Detach"
        onConfirm={() => {
          if (detachTarget)
            detach.mutate({ id: catalog.id, engineId: detachTarget.engineId });
          setDetachTarget(null);
        }}
      />
    </div>
  );
}
