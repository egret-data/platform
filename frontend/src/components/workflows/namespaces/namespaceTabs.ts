import type { WorkflowNamespaceDetailDto } from "@/api/generated/model";

/**
 * The namespace detail's tabs, in Kestra OSS's order (W7d spec §4.2, the
 * user's ruling of 2026-10-14). The id is the URL's `:tab`.
 */
export const NAMESPACE_TABS = ["overview", "flows", "executions", "dependencies", "files", "kv"] as const;
export type NamespaceTab = (typeof NAMESPACE_TABS)[number];

export const NAMESPACE_TAB_LABELS: Record<NamespaceTab, string> = {
  overview: "Overview",
  flows: "Flows",
  executions: "Executions",
  dependencies: "Dependencies",
  files: "Files",
  kv: "KV",
};

export const isNamespaceTab = (t: string | undefined): t is NamespaceTab =>
  (NAMESPACE_TABS as readonly string[]).includes(t ?? "");

/**
 * What every tab body receives from the frame (`NamespaceDetail.tsx`), which
 * has already read the namespace: a body never renders for a namespace the
 * server does not know.
 *
 * `namespace` is the namespace as the server spells it, decoded from the URL.
 * One with an empty segment (`a..b`) is listable and has a detail, but W9b's
 * file routes and W9c's KV routes refuse it with 400; the Files and KV bodies
 * show that refusal.
 */
export interface NamespaceTabProps {
  ws: string;
  namespace: string;
  /** `GET …/namespaces/{ns}`'s answer: the namespace's own counts. */
  detail: WorkflowNamespaceDetailDto;
  /**
   * The viewer holds `workflow:manage` (`detail.can_manage`, the PDP decision
   * the write routes make). Without it a tab shows everything and disables
   * each write with its reason (W7d spec §4.5); the routes still check.
   */
  canManage: boolean;
}

/**
 * A namespace's detail page at `tab`. The namespace is one path segment,
 * encoded; its dots stay as they are, and none is a bare `.` or `..` (a
 * namespace never starts with a dot).
 */
export function namespaceHref(ws: string, namespace: string, tab: NamespaceTab = "overview"): string {
  return `/explore/${ws}/workflows/namespaces/${encodeURIComponent(namespace)}/${tab}`;
}
