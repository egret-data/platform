import { TriangleAlert } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { WORKER_OFF_TEXT } from "@/components/workflows/workerOff";

/**
 * The worker-off notice (KI-013) on the flow page, the Run form, the
 * executions list and an execution that has not ended. A `status`, not an
 * `alert`, for the trigger banners' reason: it describes the page as it loads.
 */
export function WorkerOffNotice({ className }: { className?: string }) {
  return (
    <Alert role="status" data-testid="worker-off-notice" className={className}>
      <TriangleAlert aria-hidden className="h-4 w-4" />
      <AlertDescription>{WORKER_OFF_TEXT}</AlertDescription>
    </Alert>
  );
}
