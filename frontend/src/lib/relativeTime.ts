import dayjs from "dayjs";
import relativeTime from "dayjs/plugin/relativeTime";

dayjs.extend(relativeTime);

/** Human "n minutes ago" for an ISO timestamp. */
export function timeAgo(iso: string): string {
  return dayjs(iso).fromNow();
}

/** Absolute local timestamp, for tooltips. */
export function fullTimestamp(iso: string): string {
  return dayjs(iso).format("YYYY-MM-DD HH:mm:ss");
}
