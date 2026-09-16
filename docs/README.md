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

The risk-parameters page carries tables generated from repository data. Its
generator owns one marker pair and rewrites only the lines between the markers:

- `src/risk-parameters.md` from `deployments/v1/*.toml`
  (`script/docs/gen-risk-parameters.py`), between
  `<!-- BEGIN GENERATED: risk-parameters (script/docs/gen-risk-parameters.py) -->`
  and `<!-- END GENERATED: risk-parameters -->`

The generator exits with an error if the marker pair is missing, so keep both
lines intact when editing the surrounding prose.

Which markets the tables show is data, not code: `script/docs/listed-markets.toml`
names every spec under `deployments/v1/` as either `listed` (offered in the
app, rendered) or `unlisted` (deprecated or not offered, skipped). The
generator exits 1 if a spec is in neither list or in both, so adding a market
spec without classifying it fails `just docs-check` in CI. When a market is
added to or removed from the app, move its name between the two lists.

Edit the prose around the markers freely. After changing a market spec or the
listing file, run `just docs-generate`; CI runs `just docs-check` and fails if
the table is stale.
