import { useState, useEffect, type ReactNode } from "react";
import { Header } from "./Header";
import { Sidebar } from "./Sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";

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

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex h-screen flex-col bg-background">
        <Header />
        <div className="flex min-h-0 flex-1">
          <Sidebar collapsed={collapsed} onToggle={() => setCollapsed((c) => !c)} />
          <main className="min-w-0 flex-1 overflow-auto p-6">{children}</main>
        </div>
        <footer className="shrink-0 border-t border-border bg-card px-4 py-2 text-center text-xs text-muted-foreground">
          © {new Date().getFullYear()} Egret
        </footer>
      </div>
    </TooltipProvider>
  );
}
