# Released contract versions

One file per release, named `<artifact>@<version>.tsv`, holding a single
tab-separated row:

```text
artifact  version  tag  asset  sha256
```

Appended to by CI when a release tag is cut — never edited by hand. `tag` and
`asset` are recorded as observed rather than derived; see `ArtifactRelease` in
`../src/ids.rs`. Everything here was recovered from code actually deployed on
NEAR mainnet, each tag pointing at the commit its WASM names in NEP-330
metadata.

`../build.rs` reads this directory, validates every row, and compiles the
catalog. Filenames are for uniqueness and browsing only: each file is
self-describing, so renaming one cannot silently change what it means.

## Why a file per release, rather than one table

One release PR can tag several contracts at once. Each tag starts its own
workflow run, branching from the same `dev` and opening its own catalog PR — so
with a shared file, the second PR to merge conflicts with the first. Ordering
rows by artifact does not fix it: single-row groups sit within git's context
window of each other, and two first-time releases both land at the end.

Distinct filenames make the conflict unrepresentable. Two releases are never
the same file, so there is nothing to merge, whatever the merge strategy.

Absences are deliberate: no NEAR vault has shipped, mocks are test scaffolding,
and universal-account 0.4.0 was built but never deployed — those bytes are test
data, in `contract/universal-account/tests/migration/`.

To read the catalog as one table:

```bash
cat contract/artifacts/releases/*.tsv | sort
```
