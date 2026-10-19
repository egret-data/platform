# Contributing to Egret Platform

Thanks for your interest in contributing! This document describes how we work.

## Workflow

1. **Open an issue first** for any non-trivial change (feature, design change, bug) and discuss the approach.
   Larger features need a design document under `docs/superpowers/specs/` reviewed in a pull request before
   implementation starts.
2. **Fork and branch.** Use a descriptive prefix: `feat/`, `fix/`, `docs/`, `chore/`, `refactor/`, `test/`.
3. **Keep pull requests focused.** One logical change per PR. Reference the issue in the PR description with
   `Closes #<number>` (not in individual commit messages).
4. **Pass CI.** Formatting, lints, tests and the secret scan must pass.
5. **Review.** At least one maintainer approval is required. PRs are squash-merged.

## Documentation is part of the feature

A PR that changes anything a user can see or configure — a feature, a UI screen, an API, a config key,
a permission, a CLI/`just` command — **must include the matching documentation**:

- User docs live in [egret-data/docs](https://github.com/egret-data/docs) (Platform section). Open the
  docs PR alongside the code PR and link it in the PR description.
- UI changes include updated screenshots (re-run the screenshot script in the docs repo).
- Internal-only changes (refactors, tests, CI) tick "not applicable" in the PR checklist.
- Reviewers block merge when a user-visible change has no docs.

English is canonical; the Vietnamese reference (`*.vi.md`) may follow in a later PR.

## Commit messages

We follow [Conventional Commits](https://www.conventionalcommits.org/):

```text
feat(workflow): add backfill for scheduled flows
fix(authz): deny when the principal has no workspace
docs: describe engine provisioning
```

## Development

Prerequisites: Rust (see `rust-toolchain.toml`), Node.js, Docker, [`just`](https://github.com/casey/just).

```bash
cp .env.example .env     # the secret key in this file is a public dev default — never reuse it
just up                  # local dependencies
just check               # format, lints and the standard checks
just serve               # run the platform
```

Run `just --list` for the other recipes (tests, kind deployment, demo data).

## Code guidelines

- Write tests for new features and bug fixes (Arrange / Act / Assert).
- Follow existing patterns; prefer readability over cleverness. Frontend work follows
  [`docs/en/frontend-rules.md`](docs/en/frontend-rules.md).
- Fail fast with specific, contextual errors.
- Never commit secrets or credentials; `gitleaks` runs in CI.
- Label Markdown code fences with a language identifier.

## License

By contributing, you agree that your contributions will be licensed under the [Apache License 2.0](LICENSE).
