import { useState, useEffect, type ReactNode } from "react";
import { Header } from "./Header";
import { Sidebar } from "./Sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useIsNarrowViewport } from "@/lib/useIsNarrowViewport";

const STORAGE_KEY = "egret:sidebar-collapsed";

export function AppShell({ children }: { children: ReactNode }) {
  const [collapsed, setCollapsed] = useState<boolean>(() => {
    try {
      return localStorage.getItem(STORAGE_KEY) === "1";
    } catch {
      return false;
    }
  });

  useEffect(() => {
    localStorage.setItem(STORAGE_KEY, collapsed ? "1" : "0");
  }, [collapsed]);

  const narrow = useIsNarrowViewport();
  // The stored preference is a desktop preference; below the breakpoint there is no room for
  // the expanded rail regardless of what the user last chose on a wide screen. Only the
  // rendered value is overridden — `collapsed`, and so localStorage, is left alone, so a phone
  // visit does not rewrite the preference the user set on their desktop.
  const effectiveCollapsed = narrow || collapsed;

  // `h-full` (not `h-screen`): the viewport-height div now lives one level up, in
  // `RequireAuth` (see its docstring) — it wraps `ImpersonationBanner` plus this
  // shell in a single `h-screen` flex column so the banner never doubles the
  // viewport height. This shell just fills whatever height that wrapper gives it.
  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex h-full flex-col bg-background">
        <Header />
        <div className="flex min-h-0 flex-1">
          <Sidebar collapsed={effectiveCollapsed} onToggle={() => setCollapsed((c) => !c)} />
          <main className="min-w-0 flex-1 overflow-auto p-6">{children}</main>
        </div>
        <footer className="shrink-0 border-t border-border bg-card px-4 py-2 text-center text-xs text-muted-foreground">
          © {new Date().getFullYear()} Egret
        </footer>
      </div>
    </TooltipProvider>
  );
}
