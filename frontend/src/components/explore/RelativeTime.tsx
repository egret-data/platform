import { timeAgo, fullTimestamp } from "@/lib/relativeTime";

export function RelativeTime({ iso, className }: { iso: string; className?: string }) {
  return <span className={className} title={fullTimestamp(iso)}>{timeAgo(iso)}</span>;
}
