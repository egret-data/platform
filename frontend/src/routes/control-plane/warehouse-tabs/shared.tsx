import { useState } from "react";
import { Check, Copy } from "lucide-react";

/**
 * Helpers shared by more than one of the warehouse tabs (SP3c-1 Task 8).
 *
 * These tabs were `WarehouseDetail.tsx`'s until the catalog detail page took
 * them over (design D1). They are UNCHANGED — each still takes the
 * `warehouseId: string` it always took, because the registry DTO carries
 * `warehouseId` for exactly the one kind of catalog that has these tabs
 * (0147's CHECK constraint) and for no other.
 *
 * `rolesKey` and `clientsKey` live here rather than in `../queryKeys` because
 * they key the LEGACY warehouse endpoints, which SP3c-2 re-gates; the registry
 * keys next door are a different family and mixing them would suggest the two
 * caches invalidate each other.
 */
export function rolesKey(whId: string) {
  return ["control-plane", "warehouses", whId, "roles"] as const;
}
export function clientsKey(whId: string) {
  return ["control-plane", "warehouses", whId, "clients"] as const;
}

/** engine → primary (teal), notebook → success (green), external → highlight (amber). */
export function consumerDot(type: string): string {
  if (type === "engine") return "bg-primary";
  if (type === "notebook") return "bg-success";
  if (type === "external") return "bg-highlight";
  return "bg-muted-foreground";
}

export function CopyButton({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      aria-label="Copy"
      className="text-muted-foreground transition-colors hover:text-foreground"
      onClick={() => {
        void navigator.clipboard?.writeText(value);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      }}
    >
      {copied ? <Check className="h-3.5 w-3.5 text-primary" /> : <Copy className="h-3.5 w-3.5" />}
    </button>
  );
}

