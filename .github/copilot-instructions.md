# Copilot Instructions

See the repository-wide guide in [AGENTS.md](../AGENTS.md).

Additional guidance for GitHub-hosted coding agents:

- Prefer minimal diffs that preserve the current module structure.
- Run the narrowest relevant Rust tests for the files you change.
- Be careful with event payload changes in `common/src/event.rs` and shared logic in `common/src/borrow.rs` and `common/src/market`.
- Do not assume integration tests can run in restricted environments; note when `near-workspaces` or local port binding is required.
