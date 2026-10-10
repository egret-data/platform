import { classTokens, stringVisitors } from "../classTokens.js";

const ARBITRARY_SIZE = /^text-\[\d+(?:\.\d+)?(?:px|rem|em)\](?:\/.+)?$/;

export default {
  meta: {
    type: "problem",
    docs: { description: "FE-VISUAL-3: font sizes come from the type scale (docs/en/frontend-rules.md)" },
    messages: {
      size: "'{{token}}' is an arbitrary font size; use the type scale: text-xs, text-sm, text-base, text-lg, text-xl, text-2xl (FE-VISUAL-3, docs/en/frontend-rules.md).",
    },
    schema: [],
  },
  create(context) {
    return stringVisitors((node, text) => {
      for (const { raw, utility } of classTokens(text)) {
        if (ARBITRARY_SIZE.test(utility)) {
          context.report({ node, messageId: "size", data: { token: raw } });
        }
      }
    });
  },
};
