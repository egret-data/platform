# Open-source launch — implementation plan

Spec: `docs/superpowers/specs/2026-10-19-open-source-launch-design.md`

## P0 Pre-public audit (read-only)
- [ ] P0.1 gitleaks over all refs → findings list
- [ ] P0.2 tracked-file inventory (scratch, logs, internal hosts/emails, default creds)
- [ ] P0.3 licence scan: cargo-deny/about, npm license-checker, `third_party/`
- [ ] P0.4 write audit report + verdict; **gate: owner approves before P2**

## P1 Hygiene (PR on `platform`)
- [ ] P1.1 LICENSE, CODE_OF_CONDUCT, SECURITY, CODEOWNERS (copy shape from engine)
- [ ] P1.2 CONTRIBUTING (+ docs-with-feature rule)
- [ ] P1.3 .github issue templates + PR template (Docs checkbox)
- [ ] P1.4 CI workflow(s) incl. docs-changed warning
- [ ] P1.5 README / README.vi.md rewrite; remove scratch files found in P0.2
- [ ] P1.6 repo CLAUDE.md docs rule

## P2 Rename + public (owner-gated)
- [ ] P2.1 owner renames org egretdb → egret-data (web UI)
- [ ] P2.2 `gh repo rename platform`; update remotes + in-repo links + engine repo links
- [ ] P2.3 final owner confirmation → `gh repo edit --visibility public`

## P3 Docs repo
- [ ] P3.1 create `egret-data/docs`, Docusaurus scaffold, EN + VI i18n
- [ ] P3.2 sections Platform / Engine, sidebars by purpose
- [ ] P3.3 GitHub Actions deploy to Pages + link check
- [ ] P3.4 import existing `docs/en|vi` content where still valid

## P4 Run, screenshot, write, backlog
- [ ] P4.1 bring up stack, seed demo data
- [ ] P4.2 Playwright screenshot script (re-runnable)
- [ ] P4.3 per feature area: walk UI, write docs page, append BACKLOG entries (no fixes)
- [ ] P4.4 consolidate BACKLOG with severity; propose next roadmap
