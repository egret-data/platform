import type { ReactNode } from "react";
import { Navigate, useParams } from "react-router-dom";
import { useAuth } from "./useAuth";
import { useMyWorkspaces } from "@/api/hooks";
import { Forbidden } from "@/routes/Forbidden";
import { ImpersonationBanner } from "@/components/layout/ImpersonationBanner";

/**
 * Outer gate for every authenticated route (`/`, `/explore/*`, `/control-plane/*`
 * in `App.tsx`). Mounts `ImpersonationBanner` HERE — above `RequireControlPlaneAccess`
 * / `RequireGlobalAdmin` / `RequirePermission` / `RequireWorkspace`, which only wrap
 * the inner route content — so the Exit affordance survives even when one of those
 * inner guards renders a bare `Forbidden`. That case is real: impersonating a target
 * with fewer permissions than the admin while sitting on a permission-gated page
 * (e.g. starting impersonation from Control Plane → Users) instantly fails the inner
 * guard on the next render, and previously tore down `AppShell` — and the banner
 * mounted inside it — leaving the admin stranded with no way back but the 30-minute
 * auto-expiry or hand-editing the URL. `ImpersonateDialog` now also navigates to `/`
 * on a successful start precisely to avoid landing here at all; this is the
 * defense-in-depth backstop for when it doesn't.
 *
 * Mounted only once (not also in `AppShell`) to avoid a double banner.
 */
export function RequireAuth({ children }: { children: ReactNode }) {
  const { me, isLoading } = useAuth();
  if (isLoading) return <div className="p-10">Loading…</div>;
  if (!me) return <Navigate to="/login" replace />;
  return (
    <div className="flex h-screen flex-col">
      <ImpersonationBanner />
      <div className="min-h-0 flex-1">{children}</div>
    </div>
  );
}

export function RequirePlatform({ children }: { children: ReactNode }) {
  const { me } = useAuth();
  if (!me || me.platform_roles.length === 0) return <Forbidden />;
  return <>{children}</>;
}

/**
 * Control Plane is global-admin only: a platform role alone (e.g. `data_steward`)
 * is NOT enough — the user must hold `platform_admin`.
 */
export function RequireGlobalAdmin({ children }: { children: ReactNode }) {
  const { me } = useAuth();
  if (!me || !me.platform_roles.includes("platform_admin")) return <Forbidden />;
  return <>{children}</>;
}

/**
 * Gate a single control-plane route on a fine-grained permission (or set of
 * permissions). `platform_admin` always passes, regardless of `perm`.
 */
export function RequirePermission({ perm, children }: { perm: string | string[]; children: ReactNode }) {
  const { me } = useAuth();
  const keys = Array.isArray(perm) ? perm : [perm];
  const ok = !!me && (me.platform_roles.includes("platform_admin") || keys.some((k) => me.permissions.includes(k)));
  if (!ok) return <Forbidden />;
  return <>{children}</>;
}

/** The read permissions that grant entry to the Control Plane area at all. */
const CONTROL_PLANE_PERMS = [
  "engine:read",
  "warehouse:read",
  "storage:read",
  "secret:read",
  "policy:read",
  "user:read",
  "group:read",
  "role:read",
  "workspace:read",
];

/**
 * Outer gate for the whole `/control-plane/*` area: `platform_admin` OR any
 * fine-grained control-plane permission may enter. Per-route guards
 * (`RequirePermission`, `RequireGlobalAdmin`) then apply within.
 */
export function RequireControlPlaneAccess({ children }: { children: ReactNode }) {
  const { me } = useAuth();
  const ok = !!me && (me.platform_roles.includes("platform_admin") || CONTROL_PLANE_PERMS.some((k) => me.permissions.includes(k)));
  if (!ok) return <Forbidden />;
  return <>{children}</>;
}

export function RequireWorkspace({ children }: { children: ReactNode }) {
  const { workspace } = useParams();
  const { data, isLoading } = useMyWorkspaces(true);
  if (isLoading) return <div className="p-10">Loading…</div>;
  if (!data?.some((w) => w.slug === workspace)) return <Forbidden />;
  return <>{children}</>;
}
