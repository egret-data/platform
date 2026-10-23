export default {
  meta: {
    type: "problem",
    docs: { description: "FE-FORM-3: known values get the shared picker, not the browser's select" },
    messages: {
      select:
        "A bare <select> is the browser's own control; use Select from src/components/ui/select.tsx (FE-FORM-3, docs/en/frontend-rules.md).",
    },
    schema: [],
  },
  create(context) {
    return {
      JSXOpeningElement(node) {
        if (node.name.type === "JSXIdentifier" && node.name.name === "select") {
          context.report({ node, messageId: "select" });
        }
      },
    };
  },
};
