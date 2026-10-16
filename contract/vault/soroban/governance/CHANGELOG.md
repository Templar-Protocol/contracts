# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Accepted `Other` approvals expire 30 days after acceptance (`check_other`, `consume_other`
  and the duplicate-submission check all honor the expiry) and can be revoked before then via
  the new `revoke_other_approved(caller, key, payload_hash)` entrypoint, admin-gated like
  `revoke_other_pending`. Storage moves to `ApprovedOtherUntil(key, hash) -> u64`; legacy
  `ApprovedOther` entries are no longer read (Halborn FIND-024).

### Fixed

- Matured proposals expire 7 days after their ready time; `accept`/`accept_kind` return `ProposalExpired`. An expired proposal stays queued, and counts toward the pending-proposal limit, until it is revoked (Halborn FIND-026).
- Lowering a per-kind timelock now waits for the longer of that kind's current delay and the timelock-config delay, instead of only the shared config delay (Halborn FIND-022).

## [1.0.1](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-governance-v1.0.0...templar-soroban-governance-v1.0.1) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))
