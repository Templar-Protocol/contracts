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

Two pages carry tables generated from repository data. Each generator owns
exactly one marker pair on its page and rewrites only the lines between the
markers:

- `src/risk-parameters.md` from `deployments/v1/*.toml`
  (`script/docs/gen-risk-parameters.py`), between
  `<!-- BEGIN GENERATED: risk-parameters (script/docs/gen-risk-parameters.py) -->`
  and `<!-- END GENERATED: risk-parameters -->`
- `src/release-log.md` from `contract/artifacts/releases/*.tsv` and
  `script/docs/release-annotations.toml` (`script/docs/gen-release-log.py`),
  between
  `<!-- BEGIN GENERATED: release-log (script/docs/gen-release-log.py) -->`
  and `<!-- END GENERATED: release-log -->`

The generator exits with an error if its marker pair is missing, so keep both
lines intact when editing the surrounding prose.

Edit the prose around the markers freely. After changing a spec, a release
row, or an annotation, run `just docs-generate`; CI runs `just docs-check`
and fails if a table is stale.
