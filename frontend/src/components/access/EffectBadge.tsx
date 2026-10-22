import { type CSSProperties } from "react";

import { StatusBadge } from "@/components/ui/status-badge";

export const allowStyle: CSSProperties = {
  backgroundColor: "color-mix(in oklab, var(--success) 16%, transparent)",
  color: "var(--success)",
  borderColor: "color-mix(in oklab, var(--success) 32%, transparent)",
};
export const denyStyle: CSSProperties = {
  backgroundColor: "color-mix(in oklab, var(--destructive) 16%, transparent)",
  color: "var(--destructive)",
  borderColor: "color-mix(in oklab, var(--destructive) 32%, transparent)",
};

/** allow/deny, as the shared status badge (FE-VISUAL-8): *Allow* / *Deny*. */
export function EffectBadge({ effect }: { effect: string }) {
  return <StatusBadge tone={effect === "deny" ? "destructive" : "success"}>{effect}</StatusBadge>;
}
