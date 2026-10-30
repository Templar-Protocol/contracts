# 1. Record architecture decisions as ADRs

Date: 2026-10-30

## Status

Proposed

## Context

The reasons behind this repository's protocol decisions live in Linear issues, PR descriptions, and
individual agents' private memory. None of those is versioned with the code, reviewable as a
decision, or visible to the next agent or engineer who proposes changing it, so settled questions
get re-argued and reversals lose their history.

Templar already records decisions elsewhere as Nygard-style ADRs managed with a patched
[adr-tools](https://github.com/npryce/adr-tools), with agents drafting through the `templar-raladr`
agent skill and humans accepting. The skill bundles that adr-tools and installs with the
[`skills`](https://github.com/vercel-labs/skills) CLI, so every repository can share one process.

This repository is public, and so is the guide that publishes `docs/src`.

## Decision

We record architectural decisions as ADRs in `docs/src/adr/` (named in `.adr-dir`), published in the
guide and listed in `docs/src/SUMMARY.md`. Format, template, statuses, and rules are Templar's
existing ones:

- An ADR is required before code for any change to production contract behavior, a protocol
  interface, wire format or event schema, funds flow, authorization, upgrades or migrations,
  liquidation, oracle inputs, or an operational assumption of a production deployment. Tooling,
  tests, CI, and documentation need none.
- Agents draft substantive ADRs with the `templar-raladr` skill, which runs a distinct Architect and
  Critic review over the committed bytes. Process decisions use plain `adr --robot new`.
- Every ADR is born `Proposed`. Only a human accepts one, by changing its status to `Accepted` in the
  PR before merging. Merging a `Proposed` ADR is possible, but in this repository we do not.
- An `Accepted` ADR is never edited; a change of mind is a new ADR that supersedes it.
- Each fact has one home: the ADR holds the decision and its rationale, the Linear issue holds the
  work state and links the ADR, and the PR implements it and cites it as "ADR NNNN".
- ADRs are public: no secrets, undisclosed vulnerabilities, or non-public partner or operational
  details.

## Alternatives

- Keep decisions in Linear and PR descriptions: unversioned, scattered, and invisible to agents.
- MADR or log4brains: heavier tooling, and a second format beside Templar's existing ADRs.
- ADRs born `Accepted`, with merge as acceptance: simpler here, but diverges from Templar's existing
  process and from the shared skill's rule that agents propose and humans accept.
- `docs/adr/` outside the guide: no `SUMMARY.md` entry per ADR, but unpublished and its cross-links
  unchecked.
- Vendor adr-tools in this repository: a second copy of the patched scripts the skill already
  bundles.

## Consequences

Substantive decisions cost a written, reviewed ADR before implementation; that is the point. Rationale
moves out of Linear and PR bodies into one reviewable, versioned place that agents read before
proposing changes.

Each ADR needs a `SUMMARY.md` entry, and parallel branches can claim the same number: CI fails on
either, and the later branch renumbers when it rebases. `mdbook-linkcheck` checks ADR cross-links when
the guide builds on `dev`, not on pull requests. Agents need `templar-raladr` installed.
