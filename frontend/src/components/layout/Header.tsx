import { Link, useLocation } from "react-router-dom";
import { Feather, LogOut, Moon, Settings, Sun } from "lucide-react";
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
import { useLogout } from "@/api/hooks";
import { WorkspaceSwitcher } from "./WorkspaceSwitcher";

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
  const isAdmin = useLocation().pathname.startsWith("/admin");
  const canAdmin = (me?.platform_roles?.length ?? 0) > 0;

  const isDark =
    theme === "dark" ||
    (theme === "system" &&
      window.matchMedia("(prefers-color-scheme: dark)").matches);

  return (
    <header className="flex h-14 shrink-0 items-center gap-3 border-b border-border bg-card px-4">
      <Link to="/" className="flex items-center gap-2">
        <span className="flex h-8 w-8 items-center justify-center rounded-lg bg-primary text-primary-foreground shadow-sm">
          <Feather className="h-4 w-4" />
        </span>
        <span className="text-base font-semibold tracking-tight">Egret</span>
      </Link>

      {!isAdmin && <WorkspaceSwitcher />}

      <div className="flex-1" />

      {canAdmin && (
        <Button
          asChild
          variant={isAdmin ? "default" : "ghost"}
          size="sm"
          className="gap-2"
        >
          <Link to="/admin">
            <Settings className="h-4 w-4" />
            Admin
          </Link>
        </Button>
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
