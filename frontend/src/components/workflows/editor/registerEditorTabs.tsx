import { Code, FileText, TriangleAlert } from "lucide-react";
import { CodeTab } from "@/components/workflows/editor/CodeTab";
import { DocsTab } from "@/components/workflows/editor/DocsTab";
import type { EditorContext } from "@/components/workflows/editor/EditorContext";
import { ProblemsBadge, ProblemsTab } from "@/components/workflows/editor/ProblemsTab";
import { CODE_TAB } from "@/components/workflows/panels/layout";
import { registerTab } from "@/components/workflows/panels/registry";

/**
 * Register the Code, Docs and Problems tabs (spec §5.2) with the panel frame,
 * under the ids the default layout names (`code`, `docs`, `problems`). Every
 * tab receives the page's {@link EditorContext}. Registering again replaces,
 * so calling this twice is harmless; importing the module registers them.
 */
export function registerEditorTabs(): void {
  registerTab<EditorContext>({
    id: CODE_TAB,
    title: "Code",
    icon: Code,
    closable: false,
    render: (ctx) => <CodeTab ctx={ctx} />,
  });
  registerTab<EditorContext>({
    id: "docs",
    title: "Docs",
    icon: FileText,
    closable: true,
    render: (ctx) => <DocsTab ctx={ctx} />,
  });
  registerTab<EditorContext>({
    id: "problems",
    title: "Problems",
    icon: TriangleAlert,
    closable: true,
    render: (ctx) => <ProblemsTab ctx={ctx} />,
    badge: (ctx) => <ProblemsBadge ctx={ctx} />,
  });
}

registerEditorTabs();
