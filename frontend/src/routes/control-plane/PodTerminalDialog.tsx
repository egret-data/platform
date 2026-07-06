import { useEffect, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

/**
 * Control Plane → Engine → Overview → pod row "Exec" action.
 *
 * Opens `GET /api/control-plane/engines/{id}/pods/{pod}/exec` as a WebSocket
 * and bridges it to an xterm.js terminal: keystrokes go out as WS text
 * frames (raw stdin bytes, per the backend's `bridge_exec`), pod stdout
 * arrives as WS **binary** frames and is written straight to the terminal.
 * A `{"resize":{"cols":N,"rows":M}}` text frame is the one frame shape the
 * backend treats specially — everything else is stdin — so it's sent on
 * open and whenever the container is resized.
 *
 * Rendered by the caller with `pod` set/null; the dialog's open state is
 * `pod != null`. Radix `Dialog` unmounts `DialogContent` when closed, so the
 * effect below naturally tears down the terminal + socket per pod — no extra
 * "did we already open" bookkeeping is needed beyond the effect's own
 * cleanup (which also protects against React StrictMode's dev-only
 * mount→cleanup→mount cycle: the first mount's WS/terminal are closed and
 * disposed before the second mount creates a fresh pair).
 */
export default function PodTerminalDialog({
  engineId,
  pod,
  onOpenChange,
}: {
  engineId: string;
  pod: string | null;
  onOpenChange: (open: boolean) => void;
}) {
  // Callback ref via state: the terminal is created only once the container
  // node has actually attached to the DOM. Using a plain useRef races the
  // effect against Radix's dialog-content mount (the ref can still be null when
  // the effect first runs, and it never retries) — keying the effect on the
  // attached node fixes that deterministically.
  const [container, setContainer] = useState<HTMLDivElement | null>(null);

  useEffect(() => {
    if (pod == null || !container) return;

    const term = new Terminal({
      convertEol: true,
      cursorBlink: true,
      fontSize: 13,
      theme: { background: "#0a0a0a" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(container);
    fit.fit();

    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${proto}//${window.location.host}/api/control-plane/engines/${engineId}/pods/${encodeURIComponent(pod)}/exec`;
    const ws = new WebSocket(url);
    ws.binaryType = "arraybuffer";

    let closed = false;

    const sendResize = () => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ resize: { cols: term.cols, rows: term.rows } }));
      }
    };

    const onData = term.onData((d) => {
      if (ws.readyState === WebSocket.OPEN) ws.send(d);
    });

    ws.onopen = () => sendResize();

    ws.onmessage = (e) => {
      if (e.data instanceof ArrayBuffer) {
        term.write(new Uint8Array(e.data));
      } else {
        term.write(e.data as string);
      }
    };

    const onCloseOrError = () => {
      if (closed) return;
      closed = true;
      term.write("\r\n[connection closed]");
    };
    ws.onclose = onCloseOrError;
    ws.onerror = onCloseOrError;

    const handleResize = () => {
      fit.fit();
      sendResize();
    };
    window.addEventListener("resize", handleResize);

    const resizeObserver = new ResizeObserver(() => handleResize());
    resizeObserver.observe(container);

    return () => {
      window.removeEventListener("resize", handleResize);
      resizeObserver.disconnect();
      onData.dispose();
      ws.onopen = null;
      ws.onmessage = null;
      ws.onclose = null;
      ws.onerror = null;
      ws.close();
      term.dispose();
    };
  }, [engineId, pod, container]);

  return (
    <Dialog open={pod != null} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-4xl">
        <DialogHeader>
          <DialogTitle className="font-mono text-sm">{pod}</DialogTitle>
          <DialogDescription>Interactive shell (bash).</DialogDescription>
        </DialogHeader>

        <div className="h-[60vh] w-full overflow-hidden rounded-md bg-[#0a0a0a] p-2">
          <div ref={setContainer} className="h-full w-full" />
        </div>
      </DialogContent>
    </Dialog>
  );
}
