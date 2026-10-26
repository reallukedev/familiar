# Security policy

Familiar gives an AI assistant control of a real Mac and is meant to be reachable from the
internet, so security reports are very welcome.

## Reporting a vulnerability

Please don't open a public issue. Report it privately through
[GitHub's private vulnerability reporting](https://github.com/reallukedev/familiar/security/advisories/new)
with what you found, how to reproduce it and what an attacker could do with it.

You'll get a reply within a week. Once a fix is released, the advisory is published with
credit to you unless you'd rather stay anonymous.

## Scope

In scope: the Familiar server (`crates/dot-home`), the agent inside each Mac
(`crates/dot-guest`), their protocol (`crates/dot-wire`), the portal (`home-web/`) and the
container setup.

Out of scope: macOS itself, the assistant clients that connect to Familiar, and anything
an assistant does with permissions its owner granted. Prompt injection that gets an
assistant to act within what the owner allowed is a known limit; a way around the
permission, approval or isolation checks is in scope.

## How Familiar is protected

The threat model, sign-in rules, encryption at rest and isolation between people are
described in [docs/home/SECURITY.md](docs/home/SECURITY.md).
