import { classTokens, stringVisitors } from "../classTokens.js";

const UTIL = "bg|text|border(?:-[trblxyse])?|ring|ring-offset|outline|fill|stroke|from|via|to|divide|placeholder|caret|accent|decoration|shadow";
const HUE = "slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose";
const PALETTE = new RegExp(`^(?:${UTIL})-(?:${HUE})-(?:50|[1-9]00|950)(?:/(?:\\d{1,3}|\\[[^\\]]+\\]|\\([^)]+\\)))?$`);
const ARBITRARY = /^[a-z-]+-\[(?:#[0-9a-fA-F]{3,8}|(?:rgba?|hsla?|oklch|oklab|color-mix)\()/;

export default {
  meta: {
    type: "problem",
    docs: { description: "FE-VISUAL-1: colours come from tokens (docs/en/frontend-rules.md)" },
    messages: {
      raw: "'{{token}}' is a raw colour; use a token class such as bg-primary or text-muted-foreground (FE-VISUAL-1, docs/en/frontend-rules.md).",
    },
    schema: [],
  },
  create(context) {
    return stringVisitors((node, text) => {
      for (const { raw, utility } of classTokens(text)) {
        if (PALETTE.test(utility) || ARBITRARY.test(utility)) {
          context.report({ node, messageId: "raw", data: { token: raw } });
        }
      }
    });
  },
};
