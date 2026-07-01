import DOMPurify from "dompurify";
import { ansiToHtml } from "./ansiToHtml";
import type { CellOutput } from "@/lib/kernel/protocol";

function MimeBundle({ data }: { data: Record<string, unknown> }) {
  const html = data["text/html"] as string | undefined;
  const png = data["image/png"] as string | undefined;
  const jpeg = data["image/jpeg"] as string | undefined;
  const plain = data["text/plain"] as string | undefined;
  if (html) return <div className="text-sm max-h-96 overflow-auto" dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(html) }} />;
  if (png) return <img alt="output" src={`data:image/png;base64,${png}`} />;
  if (jpeg) return <img alt="output" src={`data:image/jpeg;base64,${jpeg}`} />;
  if (plain) return <pre className="text-sm whitespace-pre-wrap" dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(ansiToHtml(plain)) }} />;
  return null;
}

export function CellOutputRenderer({ outputs }: { outputs: CellOutput[] }) {
  if (!outputs?.length) return null;
  return (
    <div className="mt-2 space-y-1 border-t px-3 py-2">
      {outputs.map((o, i) => {
        switch (o.type) {
          case "stream":
            return <pre key={i} className={`text-sm whitespace-pre-wrap ${o.name === "stderr" ? "text-red-600" : ""}`}
              dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(ansiToHtml(o.text)) }} />;
          case "execute_result":
          case "display_data":
            return <MimeBundle key={i} data={o.data} />;
          case "error":
            return <pre key={i} className="text-sm text-red-600 whitespace-pre-wrap"
              dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(`${o.ename}: ${o.evalue}\n` + ansiToHtml(o.traceback.join("\n"))) }} />;
          case "truncated":
            return <div key={i} className="text-xs text-muted-foreground">… output truncated ({o.dropped} dropped)</div>;
        }
      })}
    </div>
  );
}
