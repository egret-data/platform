import { Card } from "@/components/ui/card";
export function Placeholder({ title }: { title: string }) {
  return (
    <Card className="p-6">
      <h1 className="text-lg font-semibold">{title}</h1>
      <p className="mt-2 text-muted-foreground">This feature arrives in a later phase.</p>
    </Card>
  );
}
