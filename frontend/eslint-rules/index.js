import noRawPalette from "./rules/no-raw-palette.js";
import noArbitraryTextSize from "./rules/no-arbitrary-text-size.js";
import noWindowConfirm from "./rules/no-window-confirm.js";

const plugin = {
  meta: { name: "eslint-plugin-egret" },
  rules: {
    "no-raw-palette": noRawPalette,
    "no-arbitrary-text-size": noArbitraryTextSize,
    "no-window-confirm": noWindowConfirm,
  },
};

export default plugin;
