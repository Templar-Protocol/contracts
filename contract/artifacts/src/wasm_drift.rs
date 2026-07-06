const GITHUB_TREE_PREFIX: &[u8] = b"https://github.com/Templar-Protocol/contracts/tree/";
const GITHUB_REV_PREFIX: &[u8] = b"git+https://github.com/Templar-Protocol/contracts?rev=";
const GITHUB_DOT_GIT_REV_PREFIX: &[u8] =
    b"git+https://github.com/Templar-Protocol/contracts.git?rev=";
const COMMIT_HASH_LEN: usize = 40;
const CANONICAL_COMMIT_HASH: &[u8; COMMIT_HASH_LEN] = b"0000000000000000000000000000000000000000";

pub(crate) fn canonicalize_nep330_source_refs(bytes: &[u8]) -> Vec<u8> {
    let mut canonical = bytes.to_vec();
    canonicalize_commit_hashes_after_prefix(&mut canonical, GITHUB_TREE_PREFIX);
    canonicalize_commit_hashes_after_prefix(&mut canonical, GITHUB_REV_PREFIX);
    canonicalize_commit_hashes_after_prefix(&mut canonical, GITHUB_DOT_GIT_REV_PREFIX);
    canonical
}

fn canonicalize_commit_hashes_after_prefix(bytes: &mut [u8], prefix: &[u8]) {
    let mut search_start = 0;

    while let Some(relative_start) = find_slice(&bytes[search_start..], prefix) {
        let hash_start = search_start + relative_start + prefix.len();
        let hash_end = hash_start + COMMIT_HASH_LEN;

        if bytes
            .get(hash_start..hash_end)
            .is_some_and(|hash| hash.iter().all(u8::is_ascii_hexdigit))
        {
            bytes[hash_start..hash_end].copy_from_slice(CANONICAL_COMMIT_HASH);
        }

        search_start = hash_start.saturating_add(1);
    }
}

fn find_slice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_nep330_source_refs_masks_only_commit_hashes() {
        let old_commit = b"5aab17955a142157871c6a7b19e4b668b1e2aff0";
        let new_commit = b"0123456789abcdef0123456789abcdef01234567";

        let old = [
            b"wasm-prefix".as_slice(),
            GITHUB_TREE_PREFIX,
            old_commit.as_slice(),
            b"-middle-".as_slice(),
            GITHUB_REV_PREFIX,
            old_commit.as_slice(),
            b"-suffix".as_slice(),
        ]
        .concat();
        let new = [
            b"wasm-prefix".as_slice(),
            GITHUB_TREE_PREFIX,
            new_commit.as_slice(),
            b"-middle-".as_slice(),
            GITHUB_REV_PREFIX,
            new_commit.as_slice(),
            b"-suffix".as_slice(),
        ]
        .concat();

        assert_ne!(old, new);
        assert_eq!(
            canonicalize_nep330_source_refs(&old),
            canonicalize_nep330_source_refs(&new)
        );
    }

    #[test]
    fn canonicalize_nep330_source_refs_preserves_non_metadata_drift() {
        let commit = b"0123456789abcdef0123456789abcdef01234567";
        let embedded = [b"wasm-a".as_slice(), GITHUB_REV_PREFIX, commit.as_slice()].concat();
        let disk = [b"wasm-b".as_slice(), GITHUB_REV_PREFIX, commit.as_slice()].concat();

        assert_ne!(
            canonicalize_nep330_source_refs(&embedded),
            canonicalize_nep330_source_refs(&disk)
        );
    }

    #[test]
    fn canonicalize_nep330_source_refs_leaves_malformed_refs_unmasked() {
        let malformed = [
            b"wasm-prefix".as_slice(),
            GITHUB_TREE_PREFIX,
            b"not-a-40-byte-hex-commit".as_slice(),
        ]
        .concat();

        assert_eq!(canonicalize_nep330_source_refs(&malformed), malformed);
    }
}
