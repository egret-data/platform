# Pre-public audit (P0) — 2026-10-19

Scope: full git history (all refs, 1103 commits, ~117 MB) of `egretdb/egret`, tool gitleaks 8.x.
Result: 29 raw findings, triaged below. **Verdict: NOT ready to publish until items A1–A3 are done.**

## Triage

| Finding | Where | Verdict |
|---|---|---|
| `EGRET_SECURITY__SECRET_KEY=<64-byte base64>` | `.env.example:30` (commit 4f98ab25, still at HEAD) | **REAL-LOOKING dev key. Must replace with placeholder; rotate anywhere it was used** |
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

## Actions required before public

- **A1** Replace `.env.example` and the plan-doc value with a placeholder + generation hint
  (`openssl rand -base64 64`). Because the old value stays in git history, **treat it as burned**:
  rotate in every environment that ever used it (dev stack, kind, any shared cluster).
  Secrets encrypted under that key must be re-encrypted. Owner decides: rotate-only (recommended,
  the key is a dev default) vs. history rewrite.
- **A2** Add `.gitleaks.toml` allowlist for the fixtures above; add gitleaks to CI.
- **A3** Run licence scan (cargo-deny, npm license-checker) — NOT yet done; `third_party/mysql_async`
  ships LICENSE-APACHE and LICENSE-MIT (compatible).

## Other inventory

- Tracked tree is clean of scratch files (top level: frontend, crates, docs, migrations, deploy,
  third_party, apps-image, tests, plugin-host, scripts, renderer, .github). Scratch files seen in the
  main checkout (`*.png`, `*.json`, `*.md` at its root) are untracked/ignored; keep them out of git.
- No `LICENSE`, `CONTRIBUTING`, etc. yet (P1).
- Maintainer personal email appears 116× in tracked files (git identity in docs/plans). Decide: keep or
  replace with a project address before public (owner).
- Large tracked data: `crates/egret-workflow/oracle-exec/traces.json` (6.6 MB) and oracle corpora (~1 MB each);
  fine, but mention in CONTRIBUTING.
- RFC1918/internal-looking hosts are test fixtures (SSRF tests). No real internal hostnames found.
