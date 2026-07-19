import { useEffect, useState } from "react";
import type { KernelConnection, KernelStatus } from "./KernelConnection";

/** Subscribe to a KernelConnection's status and re-render on change. */
export function useKernelStatus(kernel: KernelConnection): KernelStatus {
  const [status, setStatus] = useState<KernelStatus>(kernel.status);
  useEffect(() => kernel.onStatusChange(setStatus), [kernel]);
  return status;
}
