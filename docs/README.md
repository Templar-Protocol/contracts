# Templar Documentation

The Templar Protocol documentation site is generated using [mdBook](https://rust-lang.github.io/mdBook/).

## Development

1. Install dependencies:

   ```bash
   cargo install mdbook
   ```

1. Start mdBook server:

   ```bash
   mdbook serve
   ```

## Generated tables

Two pages carry tables generated from repository data, between
`<!-- BEGIN GENERATED -->` / `<!-- END GENERATED -->` markers:

- `src/risk-parameters.md` from `deployments/v1/*.toml`
  (`script/docs/gen-risk-parameters.py`)
- `src/release-log.md` from `contract/artifacts/releases/*.tsv` and
  `script/docs/release-annotations.toml` (`script/docs/gen-release-log.py`)

Edit the prose around the markers freely. After changing a spec, a release
row, or an annotation, run `just docs-generate`; CI runs `just docs-check`
and fails if a table is stale.
