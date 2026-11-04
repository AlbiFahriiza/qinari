# Contributing to Qinari

Thanks for your interest in improving Qinari. This repo is intentionally
small and opinionated; the fastest way to get a change merged is to keep it
focused.

## Development setup

1. Fork and clone the repository.
2. Use Node.js 24 (matching `engines` used in CI).
3. `npm install`
4. `npm run dev` and open http://localhost:3000

## Before you open a pull request

Run the same gates CI runs:

```bash
npm run typecheck
npm run lint
npm run test
npm run format
npm run build
```

All of them must pass. New behavior needs tests that fail without the change
and pass with it.

## Commit style

Use [Conventional Commits](https://www.conventionalcommits.org):

- `feat:` new user-facing behavior
- `fix:` bug fix
- `chore:` tooling, deps, CI
- `docs:` documentation only
- `refactor:` no behavior change

## Pull requests

- One concern per PR. If it fixes an issue, reference it in the description.
- Fill out the PR template checklist honestly.
- Small PRs get reviewed first. If a refactor is needed to enable your
  change, propose it in a separate PR.

## Reporting bugs

Open a [bug report](.github/ISSUE_TEMPLATE/bug_report.md) with reproduction
steps. Security issues go through the process in
[`SECURITY.md`](SECURITY.md), not public issues.
