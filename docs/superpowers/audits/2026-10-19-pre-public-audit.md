# Pre-public audit (P0) — 2026-10-19

Scope: full git history (all refs, 1103 commits, ~117 MB) of `egretdb/egret`, tool gitleaks 8.x.
Result: 29 raw findings, triaged below. **Verdict (updated after follow-up): no real secret found; secrets and licence gates pass. Remaining gate is P1 hygiene + owner's final confirmation.**

## Triage

| Finding | Where | Verdict |
|---|---|---|
| `EGRET_SECURITY__SECRET_KEY=<64-byte base64>` | `.env.example:30`, `egret-config` test | Documented PUBLIC dev key (comment in file says so), encrypts only the OIDC state cookie; also a test fixture. Initial triage over-estimated this. Decision (owner): rotate-only — any non-dev deployment must use its own key; no history rewrite |
| `SECRET_PGPASS` base64 (decodes to a `w5b…Pw42` test password) | `crates/egret-workflow/oracle-exec/docker-compose.w5b.yml` | Throw-away test compose password; acceptable, rename to obvious placeholder |
| `--user egret:egretsecret` (curl) | 7 e2e files, `frontend/tests/e2e/README.md` | Local dev MinIO/RustFS credentials; acceptable, document as dev-only |
| `curl -u …` | docs/plans/2026-07-04-trino-auth-ingress.md | Doc example; acceptable |
| `connection-password: "0b3f5c0e-1111-…"` | `engines.rs`, docs plan | Obvious fixture UUID; acceptable |
| `client_secret: "cs_live_epsilon_1c8b"` | `ClientCreate.test.tsx` | Test fixture; acceptable (rename `cs_live_` prefix to avoid scanner noise) |
| `-----BEGIN PRIVATE KEY-----` | `SecretDataFields.tsx` (placeholder text), `engines.rs` (`BBBB` fixture) | Not keys; allowlist |
| RSA PKCS#1 test key | `third_party/mysql_async/src/opts/rustls_opts.rs` | Upstream mysql_async test fixture; allowlist |
| `"key": "ion_symbol_pt1h"`, `key: …\n` | `w9c-kv.json`, `kestra-flow-schema.json` | Data/schema, not secrets; allowlist |
| `EGRET_SECURITY__SECRET_KEY=` in plan doc | docs/en/plans/2026-07-01-oidc-default-dev-e2e.md | Same key as A1; scrub |
| `EgretPluginHostTest.java` `SECRET = "s3cr3t-pw-zq9.invalid"` | plugin-host test | Fixture; acceptable |

## Actions (status)

- **A1 DONE (decision)** Rotate-only: owner's non-dev environments must not reuse the dev key. Dev default stays so `cp .env.example .env` keeps working.
- **A2 DONE** `.gitleaks.toml` allowlist added; working-tree scan: no leaks. Gitleaks goes into CI (P1).
- **A3 DONE** Licences: `cargo deny check licenses` OK with `deny.toml` (3 own crates were missing `license.workspace = true`, fixed); npm production deps all permissive (MIT 209, ISC, Apache-2.0, BSD-3, 0BSD, CC0, MPL-2.0 OR Apache-2.0, Python-2.0). `third_party/mysql_async` ships Apache + MIT.
- Owner decision: keep maintainer email in tracked files.

## Other inventory

- Tracked tree is clean of scratch files (top level: frontend, crates, docs, migrations, deploy,
  third_party, apps-image, tests, plugin-host, scripts, renderer, .github). Scratch files seen in the
  main checkout (`*.png`, `*.json`, `*.md` at its root) are untracked/ignored; keep them out of git.
- No `LICENSE`, `CONTRIBUTING`, etc. yet (P1).
- Maintainer personal email appears 116× in tracked files — owner chose to keep.
- Large tracked data: `crates/egret-workflow/oracle-exec/traces.json` (6.6 MB) and oracle corpora (~1 MB each);
  fine, but mention in CONTRIBUTING.
- RFC1918/internal-looking hosts are test fixtures (SSRF tests). No real internal hostnames found.
