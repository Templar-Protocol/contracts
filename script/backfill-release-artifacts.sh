#!/usr/bin/env bash
# One-off: publish every contract version that was actually deployed to NEAR
# mainnet as a GitHub Release, so the fetch path has something to fetch.
#
# The bytes come from the chain, not from this repository — the WASM a contract
# is *running* is the only authoritative record of what was released. Version
# numbers in Cargo.toml were repeatedly bumped without ever shipping (market
# reached 1.4.0 while 1.3.0 was the newest deployment), so they cannot be used
# to decide what a release is.
#
# Each tag is created at the commit the deployed WASM names in its own NEP-330
# `contract_source_metadata`, NOT at the tip of a branch. That is what makes
# these releases reproducible: `git checkout <tag>` lands exactly on the source
# the bytes were built from, so a rebuild in the recorded sourcescan image
# matches byte-for-byte. It also *rescues* four versions whose build commits
# currently survive only on a branch — delete that branch and the deployed code
# becomes permanently unverifiable, including for nearblocks.io.
#
# Idempotent: existing tags, releases, and assets are reused or replaced, so a
# partial run can simply be repeated.
#
# THIS PUSHES TAGS AND PUBLISHES RELEASES. Run it once, deliberately, with
# maintainer credentials. DRY_RUN=1 prints what it would do and touches nothing.
#
#   DRY_RUN=1 ./script/backfill-release-artifacts.sh   # inspect first
#   ./script/backfill-release-artifacts.sh
set -euo pipefail

SCRIPT_DIR=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

MANIFEST="script/backfill/released-versions.tsv"
RPC="${NEAR_RPC_URL:-https://rpc.mainnet.fastnear.com}"
DRY_RUN="${DRY_RUN:-}"

run() {
  if [[ -n "$DRY_RUN" ]]; then echo "  [dry-run] $*"; else "$@"; fi
}

for tool in gh jq curl; do
  command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 1; }
done
gh auth status >/dev/null 2>&1 || { echo "run 'gh auth login' first" >&2; exit 1; }

staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT

failures=0
while IFS=$'\t' read -r package version commit sha account; do
  [[ -n "$package" && "$package" != \#* ]] || continue

  tag="${package}-v${version}"
  target="${package//-/_}"
  # The catalog's target name is the package name with dashes swapped, except
  # where they diverge; ask the crate rather than guessing.
  target=$(cargo run --quiet -p templar-contract-artifacts --features fetch \
    --bin fetch-artifacts -- --print-assets | awk -v t="$tag" '$1==t {print $3; exit}')
  if [[ -z "$target" ]]; then
    echo "::error::${tag} is in the manifest but not in the catalog (ids.rs)" >&2
    failures=$((failures + 1))
    continue
  fi
  asset="${target}-${version}.wasm"

  echo "${tag}  (from ${account})"

  # 1. The bytes, straight from the account that is running them.
  curl -sf "$RPC" -H 'Content-Type: application/json' -d "$(jq -nc \
      --arg a "$account" '{jsonrpc:"2.0",id:"1",method:"query",params:{
        request_type:"view_code",finality:"final",account_id:$a}}')" \
    | jq -r '.result.code_base64' | base64 -d > "${staging}/${asset}"

  actual=$(sha256sum "${staging}/${asset}" | cut -d' ' -f1)
  if [[ "$actual" != "$sha" ]]; then
    echo "::error::${tag}: ${account} is running ${actual}, manifest says ${sha}." >&2
    echo "  The account was upgraded since the manifest was built; re-derive it." >&2
    failures=$((failures + 1))
    continue
  fi
  echo "  bytes verified: ${actual}"

  # 2. The tag, at the commit the bytes were built from.
  if ! git cat-file -e "${commit}^{commit}" 2>/dev/null; then
    echo "::error::${tag}: source commit ${commit} is not in this clone." >&2
    echo "  Fetch all branches first — without it the release is unverifiable." >&2
    failures=$((failures + 1))
    continue
  fi
  if git rev-parse -q --verify "refs/tags/${tag}" >/dev/null; then
    existing=$(git rev-parse "refs/tags/${tag}^{commit}")
    if [[ "$existing" != "$commit" ]]; then
      echo "::error::${tag} already points at ${existing}, not the build commit ${commit}." >&2
      echo "  Delete the tag deliberately if it was created by an earlier, wrong backfill." >&2
      failures=$((failures + 1))
      continue
    fi
    echo "  tag already at the build commit"
  else
    echo "  tagging ${commit:0:12}"
    run git tag "$tag" "$commit"
  fi
  run git push origin "refs/tags/${tag}"

  # 3. The release, and the asset on it.
  if gh release view "$tag" >/dev/null 2>&1; then
    echo "  release exists"
  else
    run gh release create "$tag" \
      --title "$tag" \
      --notes "\`${target}\` ${version}, as deployed on NEAR mainnet at \`${account}\`.

Built from [\`${commit}\`](../../tree/${commit}), which this tag points at.
Reproduce from a fresh clone:

\`\`\`bash
git checkout ${tag}
cargo near build reproducible-wasm --manifest-path <contract_path>/Cargo.toml
\`\`\`

\`contract_path\` and the exact build image are recorded in the WASM's own
NEP-330 \`contract_source_metadata\` — read them back with
\`near view ${account} contract_source_metadata\`.

SHA-256: \`${sha}\`"
  fi
  run gh release upload "$tag" "${staging}/${asset}" --clobber
  echo "  uploaded ${asset}"
done < "$MANIFEST"

if (( failures > 0 )); then
  echo
  echo "${failures} release(s) failed; nothing further was attempted for them." >&2
  exit 1
fi

echo
echo "Done. Verify the fetch path end to end with:"
echo "  cargo run -p templar-contract-artifacts --features fetch --bin fetch-artifacts"
echo "  ./script/check-artifact-drift.sh"
