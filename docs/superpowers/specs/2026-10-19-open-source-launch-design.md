# Open-source launch: org rename, public repo, docs site

Date: 2026-10-19 · Status: approved by owner (chat), not yet implemented

## Goal

Make the platform repo a standards-compliant public open-source project and give
platform + engine one documentation site, written from a real running app.

## Scope (4 independent packages, order P0/P1 → P2 → P3 → P4)

| # | Package | Reversible? |
|---|---|---|
| P0 | Pre-public audit: secrets in history, licences, scratch files | read-only |
| P1 | OSS hygiene of the platform repo (modelled on `egretdb/engine`) | yes (PR) |
| P2 | Rename org `egretdb` → `egret-data`, repo `egret` → `platform`, make public | **no** |
| P3 | Repo `egret-data/docs`: Docusaurus site, Platform + Engine sections, GitHub Pages | yes |
| P4 | Run app, screenshot, write docs; log defects/gaps to a backlog (no fixes) | yes |

Plus a standing rule: feature PRs must carry docs (P1 template + CONTRIBUTING + CLAUDE.md).

## P0 audit

- Scan full git history for secrets (gitleaks, all refs). Any hit: STOP, report to owner;
  owner chooses rotate-only vs history rewrite. Never publish before this is clean.
- Inventory tracked files that should not ship: scratch files, logs, generated artefacts,
  `third_party/`, internal URLs/emails/hostnames, default credentials in docs/scripts.
- Licence check of Rust (cargo-deny/about) and npm dependencies and `third_party/`
  against Apache-2.0 (the engine repo's licence).
- Output: `docs/superpowers/audits/2026-10-19-pre-public-audit.md` with findings + verdict.

## P1 hygiene (files)

`LICENSE` (Apache-2.0), `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `CODEOWNERS`,
`.github/ISSUE_TEMPLATE/{bug_report,feature_request,config}.yml`,
`.github/PULL_REQUEST_TEMPLATE.md` (with a **Docs** checkbox), CI workflow(s),
README rewrite (badges, what/why, quickstart, link to docs; README.vi.md kept).
Bilingual convention stays: English canonical, Vietnamese reference.

## P2 rename + public (owner-confirmed at each step)

1. Owner renames the org in the GitHub web UI (API cannot). 
2. `gh repo rename platform`; update remotes, in-repo links, CI, badges, image refs.
3. Final confirmation, then `gh repo edit --visibility public`.
GitHub redirects old URLs; `engine` repo links updated too.

## P3 docs site

Repo `egret-data/docs`, Docusaurus (StarRocks model: sidebar by purpose, i18n, versioning),
two top-level sections **Platform** and **Engine**. EN canonical, VI via i18n.
Deploy by GitHub Actions to `https://egret-data.github.io/docs/`; link checker in CI.
Reference study: StarRocks (Docusaurus, in-repo docs), Trino (Sphinx, in-repo),
RisingWave (Mintlify, separate docs repo).

## P4 screenshots + docs + backlog

Use the local stack (`just reset-testdata`, `just seed-demo`) and Playwright with a
re-runnable screenshot script. Walk each feature area; per area write the docs page and
append to `BACKLOG` (bugs, UX problems, missing features, severity). No fixes in this
package. The backlog feeds the next roadmap.

## Docs-with-feature rule

PR template checkbox, CONTRIBUTING section, repo CLAUDE.md line, and a non-blocking CI
warning when `frontend/` or `crates/` change without a docs change.

## Risks

- Secret/PII in history (P0 gate). - Public cache is permanent once public.
- Screenshots leak seed data/emails: use only seed data, review each image.
- Docs drift: screenshot script + docs-with-feature rule.
