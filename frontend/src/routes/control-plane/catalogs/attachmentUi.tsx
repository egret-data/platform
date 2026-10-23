/**
 * The pieces two screens share, because an attachment is a `(catalog, engine)`
 * pair and egret now has a page for each end of it: Control Plane → Catalogs →
 * detail → **Engines** (`EnginesTab.tsx`) and Control Plane → Engines → detail
 * → **Catalogs** (`EngineDetail.tsx`).
 *
 * They render the same rows from opposite directions, so the badge, the
 * Applied cell and the three mutations live here rather than twice. The "a 201
 * can carry `applyStatus: failed`" rule in particular is one that must not be
 * written down twice: a second copy is a second thing to forget.
 *
 * What does NOT live here is the wording. Each screen names the other end of
 * the pair ("this engine's primary catalog" vs "this catalog is that engine's
 * primary"), and a shared sentence would have to be vague in both places.
 *
 * NEITHER DOES THE BLOCKED-REASON RULE, and that is a statement about the DATA,
 * not about taste (whole-branch review, F1). Two different rules share the
 * Trino name, and the two screens can see different halves of them:
 *
 *   - THIS catalog's own stored config not parsing blocks Retry wherever it is
 *     attached, on either engine kind — rendering one catalog fails the same
 *     way regardless of who else is on the engine. `configError` is
 *     `RegistryCatalogDto`'s own field (SP3c-2 Task 3 moved it off the legacy
 *     `GET /engines/{id}/catalogs`, which `EngineDetail` used to be the only
 *     reader of), so `EnginesTab` now knows this one directly from `catalog`.
 *   - a SIBLING catalog on the same Trino engine not parsing blocks Retry AND
 *     Detach too, because a Trino apply rebuilds that engine's WHOLE
 *     ConfigMap. That is a fact about the engine's other attachments, which
 *     `EnginesTab` (one catalog, many engines) still cannot see —
 *     `catalog-retry-scope-note` below says so once instead of inventing a
 *     per-row reason it cannot compute. `EngineDetail` (one engine, many
 *     catalogs) computes this half.
 *
 * So `AttachmentActions` takes both reasons as REQUIRED props:
 * a caller that cannot compute one passes `null` deliberately, in the place a
 * reader will look, rather than omitting it and leaving "not applicable" and
 * "forgotten" spelled identically. `EnginesTab` says out loud, once, what its
 * `null` costs.
 */
import { RefreshCw, Unlink } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import {
  useControlPlaneCatalogRegistryDetachCatalog,
  useControlPlaneCatalogRegistryRegistryAttachCatalog,
  useControlPlaneCatalogRegistryRetryCatalogApply,
} from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { CatalogAttachmentDto } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { Button } from "@/components/ui/button";
import { catalogKey, catalogsQueryKey } from "../queryKeys";
import { StatusBadge as SharedStatusBadge } from "@/components/ui/status-badge";

/**
 * Is this engine a StarRocks one?
 *
 * The whole Trino/StarRocks asymmetry in the blocked reasons below turns on it,
 * and both screens now ask the question, so it is asked in one place. Phrased
 * negatively-by-default on purpose: an engine type egret does not recognise is
 * treated as Trino, which is the CAUTIOUS answer — a Trino Retry is the one
 * that can be refused for a sibling's sake, so an unknown type inherits the
 * warning rather than the silence.
 */
export const isStarRocksType = (t: string): boolean => t.toLowerCase() === "starrocks";

/**
 * The three `applyStatus` values, each with its own badge and its own meaning.
 *
 * The words come from `CatalogAttachmentDto.applyStatus`'s own JSDoc, which is
 * where they reach the client at all — a response description does not. The
 * three variants are deliberately distinct: one badge with three captions
 * makes `failed` read as calmly as `applied`.
 */
export function StatusBadge({ a }: { a: CatalogAttachmentDto }) {
  if (a.applyStatus === "failed")
    return (
      <SharedStatusBadge tone="destructive" title={a.applyError ?? undefined}>
        failed
      </SharedStatusBadge>
    );
  if (a.applyStatus === "pending")
    return (
      <SharedStatusBadge
        tone="highlight"
        title="egret has recorded this attachment and has not yet been able to tell the engine. A StarRocks FE takes 5-10 minutes to come up after create; the bootstrap or Retry converges it."
      >
        pending
      </SharedStatusBadge>
    );
  if (a.applyStatus === "applied") return <SharedStatusBadge tone="success">applied</SharedStatusBadge>;
  // `applyStatus` is generated as `string`, not an enum, so this arm is
  // reachable. It renders the value egret actually sent, neutrally: falling
  // through to the `applied` badge would be a false green asserting a property
  // nothing measured — and nothing would even hint that the value was
  // unrecognised (SP3c review F5).
  return (
    // Shown exactly as sent (no sentence case): a node, not a string.
    <SharedStatusBadge tone="neutral" className="italic">
      <>{a.applyStatus}</>
    </SharedStatusBadge>
  );
}

/**
 * The Applied cell.
 *
 * `applied_at` is NOT cleared by `set_apply_failed`
 * (`egret-catalog-registry/src/store.rs`), so a row that applied once and later
 * failed still carries a timestamp — which is worth showing and must not be
 * shown bare, because bare it reads as "this is live right now". And a row with
 * no timestamp is only "waiting" while it is `pending`: saying it on a `failed`
 * row makes the row contradict its own badge (SP3c review F4).
 */
export function appliedCell(a: CatalogAttachmentDto): string {
  if (a.appliedAt) {
    const when = new Date(a.appliedAt).toLocaleString();
    return a.applyStatus === "applied" ? when : `last applied ${when}`;
  }
  if (a.applyStatus === "pending") return `waiting since ${new Date(a.createdAt).toLocaleString()}`;
  if (a.applyStatus === "failed") return "never applied";
  return "—";
}

/**
 * One attachment row's Retry + Detach, with the reason under any control that
 * cannot work.
 *
 * A blocked reason both disables the button and is rendered as text: the
 * server is still the refusal point, but a disabled button whose reason is on
 * screen beats a refusal collected after typing a confirmation word — and a
 * disabled button with NO reason is the worst of the three.
 *
 * Retry is offered only on a row that is not `applied`, because re-applying an
 * applied catalog is the one case where the button measures nothing.
 *
 * Both reasons are REQUIRED, with no default (whole-branch review, F1). They
 * used to default to `null`, and the catalog-side screen — which cannot see the
 * engine-scoped Trino rule at all, see this module's header — supplied neither;
 * an omission and a considered "this screen cannot know" looked the same at
 * every call site and in every test. `null` now has to be typed out.
 */
export function AttachmentActions({
  a,
  retryBlockedReason,
  detachBlockedReason,
  busy = false,
  onRetry,
  onDetach,
}: {
  a: CatalogAttachmentDto;
  retryBlockedReason: string | null;
  detachBlockedReason: string | null;
  busy?: boolean;
  onRetry: () => void;
  onDetach: () => void;
}) {
  const showRetry = a.applyStatus !== "applied";
  // Deduplicated: one reason can block both buttons, and saying it twice under
  // one row reads as two different problems.
  const reasons = [...new Set([showRetry ? retryBlockedReason : null, detachBlockedReason])].filter(
    (r): r is string => !!r,
  );
  return (
    <div className="flex flex-col items-end gap-1">
      <div className="flex items-center justify-end gap-2">
        {showRetry && (
          <Button
            variant="outline"
            size="sm"
            onClick={onRetry}
            disabled={busy || retryBlockedReason !== null}
          >
            <RefreshCw className="h-3.5 w-3.5" />
            Retry
          </Button>
        )}
        <Button
          variant="outline"
          size="sm"
          onClick={onDetach}
          disabled={busy || detachBlockedReason !== null}
        >
          <Unlink className="h-3.5 w-3.5" />
          Detach
        </Button>
      </div>
      {reasons.map((r) => (
        <span key={r} className="text-xs text-muted-foreground">
          {r}
        </span>
      ))}
    </div>
  );
}

/**
 * Attach, detach and Retry, wired once.
 *
 * All three go through the **registry** routes (`catalog_registry:update`),
 * never the engine-mounted legacy attach (`engine:update`): a screen that
 * mixed them would hand one persona two buttons of which exactly one works,
 * which is the permission split SP3c-1 Task 1b closed.
 *
 * Every success invalidates BOTH caches that describe this attachment set —
 * the catalog's own detail query and the registry list — and the keys are
 * imported, never written out: a hand-written triple matches no cache entry at
 * all and every test stays green while the screen shows the old rows. The
 * catalog id comes from the mutation's own variables rather than from a
 * closure, so the invalidation cannot drift from the row that was clicked.
 *
 * `onDone` is whatever the calling screen must forget afterwards — the Engines
 * tab drops its Test verdict, which was measured against an attachment set
 * that no longer exists.
 */
export function useAttachmentActions({ onDone }: { onDone?: () => void } = {}) {
  const queryClient = useQueryClient();

  const settle = (catalogId: string) => {
    void queryClient.invalidateQueries({ queryKey: catalogKey(catalogId) });
    void queryClient.invalidateQueries({ queryKey: catalogsQueryKey });
    onDone?.();
  };

  const attach = useControlPlaneCatalogRegistryRegistryAttachCatalog<ApiError>({
    mutation: {
      onSuccess: (res, vars) => {
        settle(vars.id);
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
      onSuccess: (_res, vars) => {
        settle(vars.id);
        toast.success("Detached.");
      },
      // Verbatim: the 400 a primary attachment returns names the only way out.
      onError: (e) => toast.error(e.message || "Could not detach."),
    },
  });

  const retry = useControlPlaneCatalogRegistryRetryCatalogApply<ApiError>({
    mutation: {
      // This route is engine-mounted (`POST /engines/{id}/catalogs/{cid}/apply`)
      // but gated `catalog_registry:update`, so `id` is the ENGINE and `cid`
      // the catalog — the opposite of attach and detach.
      onSuccess: (_res, vars) => {
        settle(vars.cid);
        toast.success("Re-applied.");
      },
      onError: (e) => toast.error(e.message || "The engine rejected the re-apply."),
    },
  });

  return { attach, detach, retry };
}
