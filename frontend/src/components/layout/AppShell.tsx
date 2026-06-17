import type { ReactNode } from "react";
import { Header } from "./Header";
import { Sidebar } from "./Sidebar";

export function AppShell({ children }: { children: ReactNode }) {
  return (
    <div className="flex h-screen flex-col bg-background">
      <Header />
      <div className="flex min-h-0 flex-1">
        <Sidebar />
        <main className="min-w-0 flex-1 overflow-auto p-6">{children}</main>
      </div>
      <footer className="shrink-0 border-t border-border bg-card px-4 py-2 text-center text-xs text-muted-foreground">
        © {new Date().getFullYear()} Egret
      </footer>
    </div>
  );
}
