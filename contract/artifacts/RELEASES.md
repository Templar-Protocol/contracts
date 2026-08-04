# Released contract versions

Under `releases/`, one file per release named `<artifact>@<version>.tsv`, holding a single
tab-separated row:

```text
artifact  version  tag  asset  sha256
```

Appended to by CI when a release tag is cut — never edited by hand. `tag` and
`asset` are recorded as observed rather than derived; see `ArtifactRelease` in
`src/ids.rs`. Everything here was recovered from code actually deployed on
NEAR mainnet, each tag pointing at the commit its WASM names in NEP-330
metadata.

`build.rs` reads this directory, validates every row, and compiles the
catalog. Filenames are for uniqueness and browsing only: each file is
self-describing, so renaming one cannot silently change what it means.

(This lives outside `releases/` on purpose: `build.rs` watches that directory,
so a doc edit in it would recompile the crate and everything downstream.)

## Why a file per release, rather than one table

The rule that matters is that a released digest is never rewritten, and one file
per release makes that a property of the filesystem: `record-release` creates
the file with `create_new`, so a replay either matches what is there or fails.
Nothing has to parse a table to find out.

It also keeps rows independent, which is how they arrive. One release PR can tag
several contracts — contracts share a build, so they ship in batches
(`1d736e62` produced three releases, `e0f3a11f` another three) — the catalog PR
stands open across batches, and a backfill can land on `dev` beside it. A shared
file would conflict on merge. `merge=union` would fix that, but GitHub ignores
user-defined `.gitattributes` when merging a PR; distinct filenames make the
conflict unrepresentable instead — the same reason towncrier-style tools give
each entry its own file.

Absences are deliberate: no NEAR vault has shipped, mocks are test scaffolding,
and universal-account 0.4.0 was built but never deployed — those bytes are test
data, in `contract/universal-account/tests/migration/`.

To read the catalog as one table:

```bash
cat contract/artifacts/releases/*.tsv | sort
```
