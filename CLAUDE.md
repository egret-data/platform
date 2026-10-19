# Egret Platform — agent notes

- **Docs rule:** any PR that changes a user-visible feature, UI screen, API, config key or permission
  must also update the docs in `egret-data/docs` (Platform section) and tick the Docs box in the PR
  template. UI changes need refreshed screenshots.
- Conventional Commits; `Closes #N` only in PR descriptions.
- Never commit secrets. `.env.example`'s `EGRET_SECURITY__SECRET_KEY` is a public dev default.
- Scratch files go to the scratchpad, never into git.
