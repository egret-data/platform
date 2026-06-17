import type { ReactNode } from "react";
import { Navigate, useParams } from "react-router-dom";
import { useAuth } from "./useAuth";
import { useMyWorkspaces } from "@/api/hooks";
import { Forbidden } from "@/routes/Forbidden";

export function RequireAuth({ children }: { children: ReactNode }) {
  const { me, isLoading } = useAuth();
  if (isLoading) return <div className="p-10">Loading…</div>;
  if (!me) return <Navigate to="/login" replace />;
  return <>{children}</>;
}

export function RequirePlatform({ children }: { children: ReactNode }) {
  const { me } = useAuth();
  if (!me || me.platform_roles.length === 0) return <Forbidden />;
  return <>{children}</>;
}

export function RequireWorkspace({ children }: { children: ReactNode }) {
  const { workspace } = useParams();
  const { data, isLoading } = useMyWorkspaces(true);
  if (isLoading) return <div className="p-10">Loading…</div>;
  if (!data?.some((w) => w.slug === workspace)) return <Forbidden />;
  return <>{children}</>;
}
