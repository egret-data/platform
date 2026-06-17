import { Link, useLocation } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { useAuth } from "@/auth/useAuth";
import { useLogout } from "@/api/hooks";
import { WorkspaceSwitcher } from "./WorkspaceSwitcher";

export function Header() {
  const { me } = useAuth();
  const logout = useLogout();
  const isAdmin = useLocation().pathname.startsWith("/admin");
  const canAdmin = (me?.platform_roles?.length ?? 0) > 0;

  return (
    <header className="flex h-14 shrink-0 items-center gap-3 border-b border-border bg-card px-4">
      <Link to="/" className="font-semibold">🐦 Egret</Link>
      {!isAdmin && <WorkspaceSwitcher />}
      <div className="flex-1" />
      {canAdmin && (
        <Link to="/admin">
          <Button variant={isAdmin ? "default" : "ghost"} size="sm">⚙️ Admin</Button>
        </Link>
      )}
      <span className="text-sm text-muted-foreground">{me?.email}</span>
      <Button variant="ghost" size="sm" onClick={() => logout.mutate()}>Logout</Button>
    </header>
  );
}
