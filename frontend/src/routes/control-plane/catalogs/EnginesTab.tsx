import { useState } from "react";
import { Link } from "react-router-dom";
import { CheckCircle2, CircleSlash, Loader2, XCircle } from "lucide-react";

import { useControlPlaneCatalogRegistryTestRegistryCatalog } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
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
import {
  AttachmentActions,
  StatusBadge as AttachmentStatusBadge,
  appliedCell,
  isStarRocksType,
  useAttachmentActions,
} from "./attachmentUi";
import { StatusBadge } from "@/components/ui/status-badge";

/**
 * The engine's OWN lifecycle status, which is a different fact from the
 * attachment's `applyStatus` and was lost when Task 8 deleted the warehouse
 * Engines tab.
 *
 * `applyStatus` answers "did this catalog reach that engine"; an engine whose
 * cluster has since failed still answers `applied`, because it did. Scanning
 * the Apply column for trouble therefore finds none on a broken engine — which
 * is why the engine's own word sits beside its name and the apply column is
 * named after the apply.
 *
 * The value is `engines.status` as the list route last stored it, NOT a live
 * probe: `ControlPlaneEngines.tsx` treats the same field as the *fallback* for
 * a per-row `getEngine` poll. Mounting that poll here would be one extra
 * request per attachment for a fact this page does not own, so this tab shows
 * the stored value and the table's footnote says that is what it is.
 *
 * The variant mapping agrees with `ControlPlaneEngines.tsx`'s `phaseVariant`
 * on all FOUR `engine_status` values this badge can receive
 * (`egret-engine-mgmt/src/store.rs`: `provisioning`, `running`, `failed`,
 * `deleting`), so the two pages do not paint the same word two colours. It did
 * not: `phaseVariant` had no `deleting` arm and fell through to `neutral`,
 * which the whole-branch review (F6) caught and that function now covers. The
 * agreement is only claimed over those four — `phaseVariant` also maps live
 * Kubernetes PHASES (`ready`, `error`, `creating`, …) that never reach an
 * `engines.status` column.
 */
function EngineHealthBadge({ status }: { status: string | null }) {
  if (status === null)
    // An attachment whose engine row is gone (Task 3's D9 path) — egret read
    // the list and this engine was not in it. Saying nothing here would read
    // as "fine".
    return (
      <Badge variant="outline" className="text-muted-foreground italic">
        engine unknown
      </Badge>
    );
  const tone =
    status.toLowerCase() === "running"
      ? "success"
      : status.toLowerCase() === "failed"
        ? "destructive"
        : status.toLowerCase() === "provisioning" || status.toLowerCase() === "deleting"
          ? "highlight"
          : "neutral";
  return <StatusBadge tone={tone}>{status}</StatusBadge>;
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
    case "query": {
      const ran = `SHOW SCHEMAS FROM ${catalogName}${
        r.engineName ? ` on ${r.engineName}` : ""
      }`;
      return {
        tone: r.ok ? "ok" : "bad",
        // The credentials clause belongs to the SUCCESS path only. On a failure
        // the statement may not have reached authentication at all — this
        // file's own `Catalog 'sales' does not exist` fixture is such a case —
        // so claiming it exercised them is a claim about an outcome that did
        // not happen (SP3c review F7).
        what: r.ok
          ? `Query check — ${ran}, which exercises the catalog's credentials.`
          : `Query check — ${ran}. It did not succeed, so nothing was shown about the catalog's credentials either.`,
      };
    }
    case "tcp": {
      const where = r.target ?? "the host and port in the stored configuration";
      return {
        tone: r.ok ? "ok" : "bad",
        // Both halves of the ternary, not just the caveat. An unconditional
        // "egret opened and closed a TCP connection" sat directly above the
        // server's "Could not reach …: Connection refused" — two sentences in
        // one box, contradicting, on the tab whose whole job is phrasing
        // outcomes accurately (SP3c review F1). Both name the target, and both
        // carry the caveat, because the limitation is a property of the CHECK
        // and not of its outcome.
        what: r.ok
          ? `Reachability check — egret opened and closed a TCP connection to ${where}. It does not verify credentials, TLS or permissions, so it cannot tell this catalog from any other process listening on that port.`
          : `Reachability check — egret could not open a TCP connection to ${where}. Even a connection that had succeeded would not verify credentials, TLS or permissions.`,
      };
    }
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

  const isAdmin = !!me && me.platform_roles.includes("platform_admin");
  const canUpdate = isAdmin || !!me?.permissions.includes("catalog_registry:update");
  // Reading the engine list, and linking to an engine page, is `engine:read` —
  // which `catalog_registry:read` does not imply. A link that 403s exactly the
  // persona the section was built for is worse than a plain identifier, so the
  // name is rendered as text instead.
  //
  // (This used to cite the warehouse link on `CatalogDetail`'s header as the
  // precedent. It no longer carries that rationale: the link was removed
  // because `/control-plane/warehouses/{id}` now redirects to the page it sat
  // on, so it would have pointed at itself — whole-branch review, F7. The rule
  // here stands on its own; the five blocked warehouse tabs on that page, and
  // `EngineDetail`'s own `catalog_registry:read` sentence, are the live
  // examples of it.)
  const canReadEngines = isAdmin || !!me?.permissions.includes("engine:read");

  const [pick, setPick] = useState("");
  const [detachTarget, setDetachTarget] = useState<CatalogAttachmentDto | null>(null);

  // `<…, ApiError>` for the same reason every other query on this page carries
  // it: the thrown value is an `ApiError` with `.message`, and the generated
  // `TError = ErrorBody` would make `.error` type-check and be `undefined`.
  // `engine:read` is a separate permission, so this can 403 on its own.
  const enginesQ = useControlPlaneEnginesListEngines<
    controlPlaneEnginesListEnginesResponse,
    ApiError
    // `enabled` follows the permission the ROUTE needs, not the one the attach
    // picker needs: a read-only operator who may read engines can now be told
    // an engine is `failed`, which is the fact this tab was missing.
  >({ query: { retry: false, enabled: canReadEngines } });

  // ONE Test, in the header, not one per row: the route decides which of the
  // three checks is possible from the stored row and the attachments, and
  // duplicating that decision per row in the client is how the two drift.
  const test = useControlPlaneCatalogRegistryTestRegistryCatalog<ApiError>();

  /**
   * Attach, detach and Retry, and the two cache invalidations every one of
   * them owes, live in `attachmentUi` — the engine's own Catalogs tab renders
   * the same pair from the other side and must not carry a second copy of the
   * "a 201 can be `failed`" rule.
   *
   * What is local is `onDone`: the last Test verdict is DROPPED, because it
   * was measured against an attachment set that no longer exists. The route
   * picks `query` over `tcp` by looking for an applied Trino attachment, so
   * attaching or detaching one can change which check would even run —
   * leaving a red "Could not reach …:5432" box above a row that has just gone
   * green is the same class of lie the `check` discriminant exists to prevent.
   */
  const { attach, detach, retry } = useAttachmentActions({ onDone: () => test.reset() });

  const attached = new Set(catalog.engines.map((a) => a.engineId));
  const visibleEngines = enginesQ.data?.status === 200 ? enginesQ.data.data : [];
  const candidates = visibleEngines.filter((e) => !attached.has(e.id));
  /**
   * `null` means egret CANNOT KNOW any engine's health here — the list needs
   * `engine:read` and this operator has not got it, or the request failed.
   * That is a different state from "the list was read and this engine was not
   * in it", which is a per-row `null` inside the map, and the two must not
   * collapse: one is a gap in the operator's permissions, the other is a
   * dangling attachment.
   */
  const engineHealth: Map<string, string> | null =
    canReadEngines && enginesQ.data?.status === 200
      ? new Map(enginesQ.data.data.map((e) => [e.id, e.status]))
      : null;
  const failures = catalog.engines.filter((a) => a.applyStatus === "failed" && a.applyError);
  /**
   * THIS catalog's own stored config, not a sibling's. `configError` is
   * catalog-scoped (`RegistryCatalogDto.configError`, SP3c-2 Task 3), so a
   * broken row blocks Retry on every engine it is attached to, on either
   * engine kind — rendering fails the same way whoever else is on the
   * engine — and this page can say so directly. The OTHER Trino rule, a
   * sibling catalog on the same engine breaking the whole rebuild, is still
   * invisible from here (see `retryBlockedReason` below and
   * `catalog-retry-scope-note`).
   */
  const retryBlockedReason = catalog.configError
    ? "egret cannot parse this catalog's stored configuration, so it cannot render it for any engine. Repair it on this catalog's Configuration tab first."
    : null;
  /**
   * The engines whose Retry and Detach have a scope wider than the row.
   * StarRocks is excluded because its apply really is one statement about one
   * catalog; an unrecognised type is NOT, for the reason `isStarRocksType`
   * gives — the cautious answer is the one that warns.
   */
  const trinoAttachments = catalog.engines.filter((a) => !isStarRocksType(a.engineType));

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
                {/* "Apply", not "Status". It is the CATALOG's state on that
                    engine; the engine's own health sits beside its name,
                    because a failed engine still reads `applied` here. */}
                <TableHead>Apply</TableHead>
                <TableHead>Applied</TableHead>
                <TableHead className="text-right">Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {catalog.engines.map((a) => (
                <TableRow key={a.engineId}>
                  <TableCell className="font-medium">
                    <div className="flex flex-wrap items-center gap-2">
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
                      {engineHealth && (
                        <EngineHealthBadge status={engineHealth.get(a.engineId) ?? null} />
                      )}
                    </div>
                  </TableCell>
                  <TableCell>
                    <Badge variant="outline">{a.engineType}</Badge>
                  </TableCell>
                  <TableCell>
                    {a.isPrimary ? <Badge variant="secondary">primary</Badge> : null}
                  </TableCell>
                  <TableCell>
                    <AttachmentStatusBadge a={a} />
                  </TableCell>
                  <TableCell className="text-sm text-muted-foreground">
                    {appliedCell(a)}
                  </TableCell>
                  <TableCell className="text-right">
                    {canUpdate && (
                      <AttachmentActions
                        a={a}
                        // Only THIS catalog's own parse failure (whole-branch
                        // review F1, narrowed by SP3c-2 Task 3): the SAME
                        // reason applies to every row, because it is a fact
                        // about the catalog, not about any one engine. A
                        // SIBLING catalog breaking a Trino engine's whole
                        // rebuild is still invisible from here — this page
                        // reads one catalog across many engines — so the
                        // button stays enabled for THAT reason and the server
                        // stays the refusal point; `catalog-retry-scope-note`
                        // below says so once instead of a per-row reason this
                        // page cannot compute.
                        retryBlockedReason={retryBlockedReason}
                        detachBlockedReason={
                          a.isPrimary
                            ? `This is ${a.engineName}'s primary catalog. Detach it by deleting the engine.`
                            : null
                        }
                        busy={retry.isPending || detach.isPending}
                        onRetry={() => retry.mutate({ id: a.engineId, cid: catalog.id })}
                        onDetach={() => setDetachTarget(a)}
                      />
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}

      {catalog.engines.length > 0 && (
        <p className="text-xs text-muted-foreground" data-testid="catalog-apply-footnote">
          <strong>Apply</strong> is this catalog&apos;s state on that engine, not the engine&apos;s
          own health: an engine that has since failed still reads <code>applied</code> here,
          because the catalog did apply before it broke.{" "}
          {engineHealth
            ? "The badge beside each engine name is that engine's lifecycle status as egret last recorded it — the engine's own page polls it live."
            : "egret cannot show you engine health here: listing engines needs the engine:read permission, which this account does not have."}
        </p>
      )}

      {/* The scope of a refusal this page cannot predict, said ONCE rather than
          per row — and only where it can apply (whole-branch review, F1). See
          the `retryBlockedReason={null}` comment above for why the button is
          enabled rather than disabled with an invented reason. */}
      {canUpdate && trinoAttachments.length > 0 && (
        <p className="text-xs text-muted-foreground" data-testid="catalog-retry-scope-note">
          Retry and Detach on a Trino engine ({trinoAttachments.map((a) => a.engineName).join(", ")}
          ) rebuild every catalog that engine has, all at once — so either can be refused because
          a <em>different</em> catalog on the same engine has a stored configuration egret cannot
          parse, and the message will name that other catalog rather than this one. This page
          shows one catalog, so it cannot tell you in advance which engines are in that state;{" "}
          {canReadEngines
            ? "the engine's own Catalogs tab lists them and says which."
            : "an operator holding engine:read can see that on the engine's own Catalogs tab."}{" "}
          A Retry refused <em>for that reason</em> changes nothing: it never reaches the engine,
          so it leaves this catalog&apos;s apply status exactly as it is. A Retry the engine
          itself rejects is a different answer, and that one is recorded here as{" "}
          <code className="font-mono">failed</code>.
        </p>
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
          {!canReadEngines || enginesQ.isError ? (
            // NOT a picker with an empty list and the word `forbidden` under
            // it. `ApiError.message` for a 403 is `Error::Forbidden`'s Display,
            // the bare word "forbidden" — it names no permission, offers no
            // route forward, and sat below a Select and an Attach button that
            // could never do anything (SP3c review F3). The controls are gone
            // and the sentence says which permission is missing, because the
            // operator this tab is built for may hold every `catalog_registry`
            // key and still not hold `engine:read`.
            //
            // `!canReadEngines` IS THE ARM THAT ACTUALLY FIRES, and it was
            // missing (SP3c-1 Task 11, found in a browser on kind). The query
            // is `enabled: canReadEngines`, so for exactly the persona F3 was
            // written for the request is never SENT — `isError` stays false,
            // `visibleEngines` stays empty, and the branch below announced
            // "There are no engines to attach this catalog to." That is a
            // claim about the PLATFORM (there were two engines) made from an
            // answer about permissions, beside a picker that could never fill.
            // It is the same distinction `engineHealth` already draws above:
            // "egret cannot know" is not "there are none". The 403 arm stays
            // because the permission can also be lost between the `me` payload
            // and the request.
            <Alert data-testid="catalog-attach-blocked">
              <AlertDescription>
                {!canReadEngines || enginesQ.error?.status === 403
                  ? "Attaching needs the list of this platform's engines, which requires the engine:read permission. Your account does not have it, so egret cannot offer you a picker here. A platform administrator can grant engine:read; until then this catalog can only be attached by someone who holds it."
                  : `Couldn't list the engines to attach to: ${
                      enginesQ.error?.message ?? "the request failed"
                    }. Detach and Retry above are unaffected.`}
              </AlertDescription>
            </Alert>
          ) : (
            <>
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
                  onClick={() =>
                    // The picker resets only on SUCCESS, and at the call site
                    // rather than in the shared hook: a failed attach that
                    // cleared the Select would make the operator find the
                    // engine again to read the error against.
                    attach.mutate(
                      { id: catalog.id, engineId: pick },
                      { onSuccess: () => setPick("") },
                    )
                  }
                  disabled={!pick || attach.isPending}
                >
                  {attach.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
                  Attach
                </Button>
              </div>
              {!enginesQ.isLoading && candidates.length === 0 && (
                <p className="text-xs text-muted-foreground">
                  {/* "Every engine … already has it" is vacuously true of an
                      empty list and reads as "all of them do" to an operator
                      who has none (SP3c review F6). */}
                  {visibleEngines.length === 0
                    ? "There are no engines to attach this catalog to."
                    : "Every engine egret can show you already has this catalog."}
                </p>
              )}
            </>
          )}
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
