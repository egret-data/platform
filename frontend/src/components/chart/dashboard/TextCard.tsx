import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/** A dashboard text card: GFM markdown. react-markdown does not render raw HTML, so this is XSS-safe. */
export function TextCard({ text }: { text: string }) {
  if (!text.trim()) return <div className="h-full p-2" />;
  return (
    <div className="dash-markdown h-full overflow-auto p-2 text-sm [&_h1]:mb-1 [&_h1]:text-lg [&_h1]:font-semibold [&_h2]:mb-1 [&_h2]:text-base [&_h2]:font-semibold [&_ul]:list-disc [&_ul]:pl-5 [&_ol]:list-decimal [&_ol]:pl-5 [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
    </div>
  );
}
