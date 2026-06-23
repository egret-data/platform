/** A dashboard heading card: a single styled line, no card chrome. */
export function HeadingCard({ text }: { text: string }) {
  return (
    <div className="flex h-full items-center px-1 text-xl font-bold text-foreground">
      {text || <span className="text-muted-foreground">Heading</span>}
    </div>
  );
}
