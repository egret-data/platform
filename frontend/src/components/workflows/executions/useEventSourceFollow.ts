import { useEffect, useRef, useState } from "react";

/**
 * Follows a server-sent-event stream (7e #2, #3): each event's `data` is JSON,
 * parsed and passed to `onEvent`. The stream ends when the server is done, and
 * an EventSource would then reconnect for ever, so the FIRST error closes it
 * and sets `fallback`: the caller's polling takes over (a finished run needs no
 * more reads anyway). Where EventSource does not exist, `fallback` is set at once.
 *
 * `live` is true while the stream is open and has not failed. `dropped` is true only for a
 * stream that was open and then failed: the caller may read once at that moment (an absent
 * EventSource is `fallback` but not `dropped`: there was never a stream to lose).
 */
export function useEventSourceFollow<T>(
  url: string,
  { enabled, onEvent }: { enabled: boolean; onEvent: (event: T) => void },
): { live: boolean; fallback: boolean; dropped: boolean } {
  const handler = useRef(onEvent);
  useEffect(() => {
    handler.current = onEvent;
  });
  // The url the stream failed on: a new url (another execution) starts afresh.
  const [failedUrl, setFailedUrl] = useState<string | null>(null);
  const supported = typeof EventSource !== "undefined";
  const dropped = enabled && failedUrl === url;
  const failed = dropped || (enabled && !supported);

  useEffect(() => {
    if (!enabled || !supported) return;
    const source = new EventSource(url, { withCredentials: true });
    source.onmessage = (e: MessageEvent<string>) => {
      let parsed: T;
      try {
        parsed = JSON.parse(e.data) as T;
      } catch {
        return;
      }
      handler.current(parsed);
    };
    source.onerror = () => {
      source.close();
      setFailedUrl(url);
    };
    return () => source.close();
  }, [url, enabled, supported]);

  return { live: enabled && !failed, fallback: enabled && failed, dropped };
}
