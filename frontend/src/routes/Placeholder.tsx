import { Construction } from "lucide-react";

export function Placeholder({ title }: { title: string }) {
  return (
    <div className="flex h-full flex-col items-center justify-center px-6 py-16 text-center">
      <div className="flex h-16 w-16 items-center justify-center rounded-full bg-muted text-muted-foreground">
        <Construction className="h-7 w-7" />
      </div>
      <h1 className="mt-5 text-lg font-semibold">{title}</h1>
      <p className="mt-2 max-w-sm text-sm text-muted-foreground">
        This feature arrives in a later phase.
      </p>
    </div>
  );
}
