import { TriangleAlert } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import type { Banner } from "@/components/workflows/triggers/triggersModel";

/**
 * The server-flag and flow banners above a trigger list (spec §4.1), from
 * `bannersFor`. A `status`, not an `alert`: they describe the page as it
 * loads, and an alert would interrupt a screen reader on every visit.
 */
export function TriggerBanners({ banners }: { banners: Banner[] }) {
  if (banners.length === 0) return null;
  return (
    <div className="grid gap-2" data-testid="trigger-banners">
      {banners.map((b) => (
        <Alert key={b.id} role="status" data-banner={b.id}>
          <TriangleAlert aria-hidden className="h-4 w-4" />
          <AlertDescription>{b.text}</AlertDescription>
        </Alert>
      ))}
    </div>
  );
}
