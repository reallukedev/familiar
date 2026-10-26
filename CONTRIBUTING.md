# Contributing

Thanks for helping. Bug reports, fixes, new app packs and docs improvements are all
welcome.

## Before you start

- For anything bigger than a small fix, open an issue first so we can agree on the
  approach.
- Security problems go through [SECURITY.md](SECURITY.md), not public issues.
- By contributing you agree that your work is licensed under the
  [GNU GPL v3.0 or later](LICENSE), like the rest of the project.

## Setting up

You need Rust 1.88 or newer and Node 22. Building `dot-guest` needs macOS; everything
else builds on Linux too.

```bash
cargo test --workspace
```

```bash
cd home-web && npm ci && npm run dev
```

Open `http://127.0.0.1:5180/?demo` for the portal with synthetic data. You don't need a
server or a Mac to work on the UI.

## Checks

CI runs the same commands, so run them before opening a pull request:

```bash
cargo fmt --all --check
```

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
```

```bash
cd home-web && npm run check && npm run test:unit && npm run test:web
```

The workspace forbids `unsafe` and denies `unwrap()` and `expect()`; handle errors with
`Result`.

## Pull requests

- Keep each pull request to one change, with tests for behavior you add or fix.
- Explain what changed and why, and how you tested it. Screenshots help for UI changes.
- Never include real credentials, personal data or recordings of a real Mac. Fixtures and
  demo data must be synthetic.

## App packs

Packs add tools for a Mac app without changing the server. See
[docs/home/PACKS.md](docs/home/PACKS.md) for the script contract and permission groups,
and the built-in packs in `crates/dot-home/packs/` for examples.
