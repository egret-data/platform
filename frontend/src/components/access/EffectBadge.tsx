import { type CSSProperties } from "react";

import { Badge } from "@/components/ui/badge";

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

/** allow/deny badge tinted with the semantic success/destructive token. */
export function EffectBadge({ effect }: { effect: string }) {
  const deny = effect === "deny";
  return (
    <Badge className="border font-normal" style={deny ? denyStyle : allowStyle}>
      {effect}
    </Badge>
  );
}
