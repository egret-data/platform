# Frontend

Before changing anything under `frontend/`, read the rulebook: [`../docs/en/frontend-rules.md`](../docs/en/frontend-rules.md) (Vietnamese reference: `../docs/vi/frontend-rules.md`). Its MUST rules are requirements; lint enforces the machine-checkable ones (`npm run lint`), and its reviewer checklist covers the rest.

- Gate: `npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`.
- Regenerate the API client with `npm run gen:api`, never bare `orval`.
- No prettier in this tree.
- Never add a file to `eslint-rules/legacy.js`; a file you fix leaves it.
