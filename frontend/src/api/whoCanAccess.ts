/**
 * Who-can-access API helper (reverse of Access explorer: object → principals).
 *
 * Thin wrapper over:
 *   GET /api/control-plane/access/who-can-access?kind={kind}&id={uuid}
 *
 * Calls `apiJson` directly (not the orval client), so the path is passed
 * WITHOUT the `/api` prefix — `apiJson`/`apiFetch` prepend it — mirroring
 * `workspaceAccess.ts` (B1).
 */
import { apiJson } from "./client";

export interface ObjectRef {
  kind: string;
  id: string;
  name: string | null;
  workspace_slug: string | null;
}

export interface AccessEntry {
  principal_type: "user" | "group";
  principal_id: string;
  principal_name: string | null;
  access: string;
  effect: string;
  /** "direct" | "workspace:{slug}" | "platform" */
  source: string;
}

export interface WhoCanAccess {
  object: ObjectRef;
  entries: AccessEntry[];
}

export async function getWhoCanAccess(kind: string, id: string): Promise<WhoCanAccess> {
  return apiJson<WhoCanAccess>(
    `/control-plane/access/who-can-access?kind=${encodeURIComponent(kind)}&id=${encodeURIComponent(id)}`,
  );
}
