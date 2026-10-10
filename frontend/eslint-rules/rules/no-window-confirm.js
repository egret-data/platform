export default {
  meta: {
    type: "problem",
    docs: { description: "FE-ACTION-1: confirm destructive actions with a dialog" },
    messages: {
      confirm: "window.confirm is not egret's confirmation; use ConfirmDeleteDialog (src/components/ui/confirm-delete-dialog.tsx) or AlertDialog (FE-ACTION-1, docs/en/frontend-rules.md).",
    },
    schema: [],
  },
  create(context) {
    const isGlobalConfirm = (node) => {
      let scope = context.sourceCode.getScope(node);
      for (; scope; scope = scope.upper) {
        const v = scope.set.get("confirm");
        if (v && v.defs.length > 0) return false;
      }
      return true;
    };
    return {
      CallExpression(node) {
        const c = node.callee;
        if (c.type === "Identifier" && c.name === "confirm" && isGlobalConfirm(c)) {
          context.report({ node, messageId: "confirm" });
        } else if (
          c.type === "MemberExpression" && !c.computed &&
          c.property.type === "Identifier" && c.property.name === "confirm" &&
          c.object.type === "Identifier" && ["window", "globalThis", "self"].includes(c.object.name)
        ) {
          context.report({ node, messageId: "confirm" });
        }
      },
    };
  },
};
