import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/** A dashboard text card: GFM markdown (XSS-safe; react-markdown does not render raw HTML).
 *  In edit mode it is an inline textarea bound to the raw markdown (so {{slug}} stays literal). */
export function TextCard({ text, editing, autoFocus, onChange }: { text: string; editing?: boolean; autoFocus?: boolean; onChange?: (text: string) => void }) {
  if (editing && onChange) {
    return (
      <textarea
        aria-label="text body"
        className="dash-nodrag h-full w-full resize-none bg-transparent p-2 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none"
        placeholder="Write text…  ({{filter}} inserts a filter value)"
        autoFocus={autoFocus}
        value={text}
        onChange={(e) => onChange(e.target.value)}
      />
    );
  }
  if (!text.trim()) return <div className="h-full p-2" />;
  return (
    <div className="dash-markdown h-full overflow-auto p-2 text-sm [&_h1]:mb-1 [&_h1]:text-lg [&_h1]:font-semibold [&_h2]:mb-1 [&_h2]:text-base [&_h2]:font-semibold [&_ul]:list-disc [&_ul]:pl-5 [&_ol]:list-decimal [&_ol]:pl-5 [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
    </div>
  );
}
