import { useEffect } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";
import { Compass, LogOut, Moon, Settings, ShieldCheck, Sun } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useTheme } from "@/components/theme-provider";
import { useAuth } from "@/auth/useAuth";
import { type AppArea, appForPath, canEnterControlPlane, canEnterGovernance } from "@/auth/appAccess";
import { useLogout } from "@/api/hooks";
import { useIsNarrowViewport } from "@/lib/useIsNarrowViewport";
import { WorkspaceSwitcher } from "./WorkspaceSwitcher";

const LAST_WORKSPACE_KEY = "egret:last-workspace";

function initials(email?: string) {
  if (!email) return "?";
  const name = email.split("@")[0];
  const parts = name.split(/[._-]+/).filter(Boolean);
  const chars = parts.length >= 2 ? parts[0][0] + parts[1][0] : name.slice(0, 2);
  return chars.toUpperCase();
}

export function Header() {
  const { me } = useAuth();
  const logout = useLogout();
  const { theme, setTheme } = useTheme();
  const location = useLocation();
  const navigate = useNavigate();
  const { workspace } = useParams<{ workspace?: string }>();
  const app = appForPath(location.pathname);
  // Same source of truth as the sidebar rule — one viewport breakpoint for the whole shell.
  // At 390px the row below is 441px of fixed-width children, which is what puts a horizontal
  // scrollbar on the document no matter how narrow the dashboard grid itself gets.
  const narrow = useIsNarrowViewport();
  // Governance and Control Plane each show for platform_admin OR anyone holding one of that
  // app's read permissions — the same predicates as RequireGovernanceAccess /
  // RequireControlPlaneAccess. Explorer is shown beside them so the three read as one
  // switcher; with neither app reachable there is nothing to switch to, so none render.
  const showGovernance = canEnterGovernance(me);
  const showControlPlane = canEnterControlPlane(me);
  const showSwitcher = showGovernance || showControlPlane;
  const entrySize = narrow ? "icon" : "sm";
  const entryClass = narrow ? undefined : "gap-2";
  const current = (area: AppArea): "page" | undefined => (app === area ? "page" : undefined);
  const variantFor = (area: AppArea): "secondary" | "ghost" => (app === area ? "secondary" : "ghost");

  // Persist the current workspace slug so Explorer can be restored from Admin
  useEffect(() => {
    if (workspace) {
      localStorage.setItem(LAST_WORKSPACE_KEY, workspace);
    }
  }, [workspace]);

  const isDark =
    theme === "dark" ||
    (theme === "system" &&
      window.matchMedia("(prefers-color-scheme: dark)").matches);

  function handleExplorer() {
    const last = localStorage.getItem(LAST_WORKSPACE_KEY);
    navigate(last ? `/explore/${last}` : "/");
  }

  return (
    <header
      data-testid="app-header"
      className={`flex h-14 shrink-0 items-center border-b border-border bg-card ${
        narrow ? "gap-2 px-2" : "gap-3 px-4"
      }`}
    >
      <Link to="/" className="flex shrink-0 items-center gap-2">
        <img src="/favicon.svg" alt={narrow ? "Egret" : ""} className="h-8 w-8 rounded-lg shadow-sm" />
        {/* The mark alone still identifies the app; the wordmark is the cheapest 55px to give
            back, and it is the only element here with no function of its own. */}
        {!narrow && <span className="text-base font-semibold tracking-tight">Egret</span>}
      </Link>

      {/* `min-w-0 flex-1` lets the workspace name truncate instead of forcing the row wider
          than the viewport — a fixed w-56 trigger cannot shrink and pushes everything right. */}
      {app === "explore" && <WorkspaceSwitcher className={narrow ? "w-auto min-w-0 flex-1" : undefined} />}

      {/* The spacer pushes the actions right. Not beside a narrow switcher: two
          `flex-1` items split the free room, and the switcher was left showing
          "De" for "Default" at 390px (KI-233). The switcher's own `flex-1`
          does the pushing there. */}
      {!(app === "explore" && narrow) && <div className="flex-1" />}

      {showSwitcher && (
        <nav aria-label="Apps" className={`flex items-center ${narrow ? "gap-2" : "gap-3"}`}>
          <Button
            variant={variantFor("explore")}
            size={entrySize}
            className={entryClass}
            aria-label="Explorer"
            aria-current={current("explore")}
            title="Explorer"
            onClick={handleExplorer}
          >
            <Compass className="h-4 w-4" />
            {!narrow && "Explorer"}
          </Button>
          {showGovernance && (
            <Button asChild variant={variantFor("governance")} size={entrySize} className={entryClass}>
              <Link to="/governance" aria-label="Governance" aria-current={current("governance")} title="Governance">
                <ShieldCheck className="h-4 w-4" />
                {!narrow && "Governance"}
              </Link>
            </Button>
          )}
          {showControlPlane && (
            <Button asChild variant={variantFor("control-plane")} size={entrySize} className={entryClass}>
              <Link
                to="/control-plane"
                aria-label="Control Plane"
                aria-current={current("control-plane")}
                title="Control Plane"
              >
                <Settings className="h-4 w-4" />
                {!narrow && "Control Plane"}
              </Link>
            </Button>
          )}
        </nav>
      )}

      <Button
        variant="ghost"
        size="icon"
        aria-label="Toggle theme"
        onClick={() => setTheme(isDark ? "light" : "dark")}
      >
        {isDark ? (
          <Sun className="h-4 w-4" />
        ) : (
          <Moon className="h-4 w-4" />
        )}
      </Button>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon"
            className="rounded-full"
            aria-label="User menu"
          >
            <Avatar className="h-8 w-8">
              <AvatarFallback className="bg-primary/10 text-xs font-medium text-primary">
                {initials(me?.email)}
              </AvatarFallback>
            </Avatar>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-56">
          <DropdownMenuLabel className="flex flex-col gap-0.5">
            <span className="text-xs font-normal text-muted-foreground">
              Signed in as
            </span>
            <span className="truncate text-sm font-medium">{me?.email}</span>
          </DropdownMenuLabel>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => logout.mutate()}>
            <LogOut className="mr-2 h-4 w-4" />
            Log out
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </header>
  );
}
