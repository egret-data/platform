/** A dashboard heading card: a single styled line, no card chrome. */
export function HeadingCard({ text }: { text: string }) {
  return (
    <div className="flex h-full items-center px-1 text-lg font-semibold text-foreground">
      {text || <span className="text-muted-foreground">Heading</span>}
    </div>
  );
}
