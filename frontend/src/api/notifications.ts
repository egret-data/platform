import { useMutation, useQueryClient } from "@tanstack/react-query";

import type { ApiError } from "./client";
import {
  alertsSmtpDelete,
  alertsSmtpPut,
  alertsSmtpTest,
  getAlertsSmtpGetQueryKey,
  useAlertsSmtpGet,
} from "./generated/alerts/alerts";
import type { SmtpConfigBody, SmtpConfigDto } from "./generated/model";

export type SmtpConfig = SmtpConfigDto;
export type TlsMode = SmtpConfigDto["tls_mode"];

/**
 * The workspace's SMTP settings, or `undefined` when none are stored.
 *
 * A 404 here is the ordinary starting state of every workspace, not a failure,
 * so callers should render an empty form rather than an error. `retry: false`
 * keeps that answer immediate instead of three round-trips slow.
 */
export function useSmtpConfig(ws: string) {
  return useAlertsSmtpGet<SmtpConfig, ApiError>(ws, {
    query: {
      enabled: Boolean(ws),
      retry: false,
      select: (r) => r.data as SmtpConfig,
    },
  });
}

export function useSmtpMutations(ws: string) {
  const qc = useQueryClient();
  const invalidate = () =>
    qc.invalidateQueries({ queryKey: getAlertsSmtpGetQueryKey(ws) });

  const save = useMutation({
    mutationFn: (body: SmtpConfigBody) => alertsSmtpPut(ws, body),
    onSuccess: invalidate,
  });
  const remove = useMutation({
    mutationFn: () => alertsSmtpDelete(ws),
    onSuccess: invalidate,
  });
  // Deliberately does NOT invalidate: a test send changes nothing, and
  // refetching would make the form flicker for no reason.
  const sendTest = useMutation({
    mutationFn: (to?: string) => alertsSmtpTest(ws, { to: to ?? null }),
  });
  return { save, remove, sendTest };
}
