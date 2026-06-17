import { Card } from "@/components/ui/card";
export function Forbidden() {
  return <div className="p-10"><Card className="p-6"><h1 className="text-lg font-semibold">403 — Forbidden</h1><p className="text-muted-foreground">You don't have access to this page.</p></Card></div>;
}
