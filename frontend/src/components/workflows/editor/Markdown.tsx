import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/**
 * Schema descriptions are Markdown (Kestra's carry headings, examples and
 * code). react-markdown renders no raw HTML, so a description cannot inject
 * markup. `inline` drops the paragraph wrapper, for a table cell.
 */
export function Markdown({ children, inline = false }: { children: string; inline?: boolean }) {
  if (inline) {
    return (
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ p: ({ children: c }) => <>{c}</> }}>
        {children}
      </ReactMarkdown>
    );
  }
  return (
    <div className="space-y-2 text-sm text-foreground [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_h5]:font-semibold [&_ol]:list-decimal [&_ol]:pl-5 [&_pre]:overflow-x-auto [&_pre]:rounded [&_pre]:bg-muted [&_pre]:p-2 [&_ul]:list-disc [&_ul]:pl-5">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{children}</ReactMarkdown>
    </div>
  );
}
