//! Download and cache released contract WASM. Enabled via the `fetch` feature.
//!
//! Bytes are GitHub Release assets, not repository content; each release in
//! `contract/artifacts/releases/` records the tag and asset it shipped as.
//! See `RELEASING.md` for why they are built at the tag's own commit.
//!
//! Every read is verified against the catalog's SHA-256 pin, including of an
//! existing cache entry — so a branch whose catalog disagrees re-downloads
//! rather than reusing wrong bytes, and offline that mismatch is an error.
//!
//! ```text
//! ${TEMPLAR_ARTIFACT_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}}/templar-contract-artifacts
//!   └── near/<cargo_target_name>/<version>/<cargo_target_name>.wasm
//! ```
//!
//! The cache is shared across worktrees and needs no locking: an entry deleted
//! mid-fetch is re-downloaded. `TEMPLAR_ARTIFACT_OFFLINE=1` restricts lookups to
//! it; `just artifacts-fetch` warms it, `just artifacts-clean` empties it.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    time::Duration,
};

use thiserror::Error;

use crate::{sha256_hex, ArtifactId, ArtifactRelease};

/// Repository whose GitHub Releases host the artifact assets.
const RELEASE_REPO: &str = "Templar-Protocol/contracts";

/// Download attempts before giving up. Assets are immutable, so a retry can
/// only ever fetch the same bytes.
const DOWNLOAD_ATTEMPTS: u32 = 3;

const RETRY_BACKOFF: Duration = Duration::from_millis(500);

/// Per-request ceiling. `reqwest`'s async client has no default timeout, so
/// without this a stalled connection hangs a CI job for its whole budget
/// instead of failing into the retry loop.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Simultaneous downloads during a prefetch. Enough to hide per-request
/// latency, low enough to stay well clear of GitHub's rate limiting.
const PREFETCH_CONCURRENCY: usize = 8;

#[derive(Debug, Error)]
pub enum FetchError {
    #[error(
        "{artifact} has no release {version}; catalogued releases: [{catalogued}]. \
         Historical versions live in contract/artifacts/releases/."
    )]
    UnknownRelease {
        artifact: ArtifactId,
        version: String,
        catalogued: String,
    },

    #[error(
        "{artifact}@{version} is not in the local cache and TEMPLAR_ARTIFACT_OFFLINE \
         is set. Run `just artifacts-fetch` with network access, or unset it."
    )]
    Offline {
        artifact: ArtifactId,
        version: String,
    },

    #[error("failed to download {url} after {attempts} attempts: {source}")]
    Download {
        url: String,
        attempts: u32,
        source: reqwest::Error,
    },

    #[error("{url} returned HTTP {status}")]
    Status { url: String, status: u16 },

    #[error(
        "{artifact}@{version} downloaded from {url} hashes to {actual}, but the \
         catalog pins {expected}. Refusing the bytes."
    )]
    HashMismatch {
        artifact: ArtifactId,
        version: String,
        url: String,
        expected: String,
        actual: String,
    },

    #[error("artifact cache I/O failed at {path}: {source}")]
    Cache {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("no cache directory: set TEMPLAR_ARTIFACT_CACHE, XDG_CACHE_HOME, or HOME")]
    NoCacheDir,

    #[error("a prefetch task failed to complete: {0}")]
    Join(String),

    #[error("could not build the HTTP client: {0}")]
    Client(String),
}

/// An exported-but-empty variable reads as `Some("")`, which would resolve the
/// cache to a *relative* path — and `clean()` would then delete `./near` out of
/// whatever directory it happened to run in. Empty means unset.
fn non_empty_path(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

fn env_path(key: &str) -> Option<PathBuf> {
    non_empty_path(std::env::var_os(key))
}

/// Directory this crate owns inside whichever cache location is in force.
///
/// Always appended, including under `TEMPLAR_ARTIFACT_CACHE` — that override
/// names the *parent*, exactly like `XDG_CACHE_HOME`. `clean()` can then only
/// ever delete a directory this crate named, so pointing the override at a
/// populated directory is harmless rather than destructive.
const CACHE_DIR: &str = "templar-contract-artifacts";

/// Root of the shared artifact cache.
pub fn cache_root() -> Result<PathBuf, FetchError> {
    let base = env_path("TEMPLAR_ARTIFACT_CACHE")
        .or_else(|| env_path("XDG_CACHE_HOME"))
        .or_else(|| env_path("HOME").map(|home| home.join(".cache")))
        .ok_or(FetchError::NoCacheDir)?;
    Ok(base.join(CACHE_DIR))
}

/// Where a given release's bytes are cached.
pub fn cache_path(artifact: ArtifactId, version: &str) -> Result<PathBuf, FetchError> {
    let target = artifact.metadata().cargo_target_name;
    Ok(cache_root()?
        .join("near")
        .join(target)
        .join(version)
        .join(format!("{target}.wasm")))
}

/// Download URL for a release's WASM asset. Both segments are read off the
/// release record, not rebuilt from a naming convention.
pub fn asset_url(release: &ArtifactRelease) -> String {
    let base = std::env::var("TEMPLAR_ARTIFACT_BASE_URL")
        .unwrap_or_else(|_| format!("https://github.com/{RELEASE_REPO}/releases/download"));
    format!("{base}/{}/{}", release.tag, release.asset)
}

/// Bytes of a released contract version, from the cache or the GitHub Release.
///
/// Verified against the catalog's SHA-256 pin before being cached or returned.
pub async fn released_bytes(artifact: ArtifactId, version: &str) -> Result<Vec<u8>, FetchError> {
    let metadata = artifact.metadata();
    let Some(release) = metadata.release(version) else {
        return Err(FetchError::UnknownRelease {
            artifact,
            version: version.to_owned(),
            catalogued: metadata
                .releases()
                .iter()
                .map(|release| release.version)
                .collect::<Vec<_>>()
                .join(", "),
        });
    };
    let expected = release.sha256;

    let path = cache_path(artifact, version)?;
    // A cached file that no longer matches its pin is treated as a corrupt
    // cache entry, not as evidence of tampering: re-download and let the
    // verification below decide.
    if let Ok(cached) = std::fs::read(&path) {
        if sha256_hex(&cached) == expected {
            return Ok(cached);
        }
    }

    if std::env::var_os("TEMPLAR_ARTIFACT_OFFLINE").is_some() {
        return Err(FetchError::Offline {
            artifact,
            version: version.to_owned(),
        });
    }

    let url = asset_url(release);
    let bytes = download(&url).await?;

    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(FetchError::HashMismatch {
            artifact,
            version: version.to_owned(),
            url,
            expected: expected.to_owned(),
            actual,
        });
    }

    // The bytes are already verified, so caching them is an optimization: a
    // concurrent clean deleting the directory costs a re-download, not this
    // call. Other cache failures still surface.
    match write_atomically(&path, &bytes) {
        Err(FetchError::Cache { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {}
        other => other?,
    }
    Ok(bytes)
}

/// Populate the cache with every release in the catalog, returning the count.
///
/// Concurrent: wall clock is per-request latency to GitHub's CDN, not
/// bandwidth. Sequentially this costs CI minutes per run.
pub async fn prefetch_all() -> Result<usize, FetchError> {
    let releases = ArtifactId::ALL
        .into_iter()
        .flat_map(|artifact| {
            artifact
                .metadata()
                .releases()
                .iter()
                .map(move |release| (artifact, release.version))
        })
        .collect::<Vec<_>>();

    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(PREFETCH_CONCURRENCY));
    let mut tasks = tokio::task::JoinSet::new();
    for (artifact, version) in releases {
        let permits = std::sync::Arc::clone(&permits);
        tasks.spawn(async move {
            // The semaphore is never closed, so acquiring cannot fail.
            let _permit = permits.acquire().await;
            released_bytes(artifact, version).await.map(|_| ())
        });
    }

    let mut cached = 0;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Ok(())) => cached += 1,
            Ok(Err(error)) => return Err(error),
            Err(join) => return Err(FetchError::Join(join.to_string())),
        }
    }
    Ok(cached)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CacheUsage {
    pub files: usize,
    pub bytes: u64,
}

/// Empty the artifact cache, reporting what was removed.
///
/// Only ever deletes inside [`CACHE_DIR`], which this crate names, so a misaimed
/// `TEMPLAR_ARTIFACT_CACHE` cannot reach anything already in that directory.
///
/// Safe to run while another worktree is fetching: the subtree is renamed aside
/// before it is walked, so a concurrent fetch rebuilds a fresh `near/`.
pub fn clean() -> Result<CacheUsage, FetchError> {
    clean_at(&cache_root()?)
}

/// [`clean`] against an explicit root, so tests need not mutate the environment.
fn clean_at(root: &Path) -> Result<CacheUsage, FetchError> {
    // Every path below resolves *through* the root, so a symlinked one puts the
    // rename and the recursive delete inside whatever it points at. Namespacing
    // only isolates us while the namespace is a real directory — and in a shared
    // parent, someone else can create it first.
    if root
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(FetchError::Cache {
            path: root.to_owned(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "cache root is a symlink; refusing to delete through it",
            ),
        });
    }

    let near = root.join("near");

    // `remove_dir_all` is a readdir/unlink/rmdir walk, so a concurrent fetch can
    // leave it with `ENOTEMPTY` or lose its own directory to `ENOENT`. Renaming
    // first is one syscall, after which the walk counts exactly what it removes.
    let doomed = root.join(format!(".near.cleaning.{}", std::process::id()));
    let removed = match std::fs::rename(&near, &doomed) {
        Ok(()) => {
            let removed = usage_of(&doomed)?;
            std::fs::remove_dir_all(&doomed).map_err(|source| FetchError::Cache {
                path: doomed,
                source,
            })?;
            removed
        }
        // Nothing cached yet: an empty cache cleans to zero rather than erroring.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => CacheUsage::default(),
        Err(source) => return Err(FetchError::Cache { path: near, source }),
    };

    // Non-recursive, and failure is fine: it only succeeds when the root holds
    // nothing but the subtree we just removed.
    let _ = std::fs::remove_dir(root);

    Ok(removed)
}

/// A missing directory totals as an empty one.
fn usage_of(dir: &Path) -> Result<CacheUsage, FetchError> {
    let mut total = CacheUsage::default();
    let mut pending = vec![dir.to_owned()];

    while let Some(current) = pending.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(FetchError::Cache {
                    path: current,
                    source,
                })
            }
        };
        for entry in entries {
            let entry = entry.map_err(|source| FetchError::Cache {
                path: current.clone(),
                source,
            })?;
            let metadata = entry.metadata().map_err(|source| FetchError::Cache {
                path: entry.path(),
                source,
            })?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                total.files += 1;
                total.bytes += metadata.len();
            }
        }
    }

    Ok(total)
}

/// Statuses worth another attempt: rate limiting and server-side faults. Every
/// other status is the server's settled answer and retrying only wastes time.
fn is_retryable(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

async fn download(url: &str) -> Result<Vec<u8>, FetchError> {
    let mut last = None;
    let mut last_status = None;
    for attempt in 0..DOWNLOAD_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(RETRY_BACKOFF * 2u32.pow(attempt - 1)).await;
        }
        match try_download(url).await {
            Ok(bytes) => return Ok(bytes),
            Err(error @ FetchError::Status { status, .. }) => {
                if !is_retryable(status) {
                    return Err(error);
                }
                last_status = Some(status);
            }
            Err(FetchError::Download { source, .. }) => last = Some(source),
            Err(other) => return Err(other),
        }
    }
    Err(match last {
        Some(source) => FetchError::Download {
            url: url.to_owned(),
            attempts: DOWNLOAD_ATTEMPTS,
            source,
        },
        // Every attempt returned a retryable status rather than a transport
        // error; report the last one the server actually gave us.
        None => FetchError::Status {
            url: url.to_owned(),
            status: last_status.unwrap_or(0),
        },
    })
}

/// One client per process, so a prefetch reuses connections instead of paying a
/// TLS handshake per asset.
fn client() -> Result<&'static reqwest::Client, FetchError> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| FetchError::Client(error.clone()))
}

async fn try_download(url: &str) -> Result<Vec<u8>, FetchError> {
    let transport = |source| FetchError::Download {
        url: url.to_owned(),
        attempts: 1,
        source,
    };

    let response = client()?.get(url).send().await.map_err(transport)?;

    let status = response.status();
    if !status.is_success() {
        return Err(FetchError::Status {
            url: url.to_owned(),
            status: status.as_u16(),
        });
    }

    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(transport)
}

/// Write via a unique temporary file and rename, so concurrent test binaries
/// racing on the same release never observe a half-written blob.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), FetchError> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let cache = |source| FetchError::Cache {
        path: path.to_owned(),
        source,
    };

    let dir = path.parent().unwrap_or(path);
    std::fs::create_dir_all(dir).map_err(cache)?;

    let temporary = dir.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("artifact"),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::write(&temporary, bytes).map_err(cache)?;
    std::fs::rename(&temporary, path).map_err(cache)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `clean_at` takes an explicit path so these need not mutate
    /// `TEMPLAR_ARTIFACT_CACHE` for the rest of the binary.
    fn scratch_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "templar-artifact-cache-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    fn seed(root: &Path, target: &str, version: &str, bytes: &[u8]) {
        let dir = root.join("near").join(target).join(version);
        std::fs::create_dir_all(&dir).expect("scratch cache directory");
        std::fs::write(dir.join(format!("{target}.wasm")), bytes).expect("scratch cache entry");
    }

    #[test]
    fn clean_removes_every_entry_and_reports_what_it_freed() {
        let root = scratch_root("removes");
        seed(&root, "templar_market_contract", "1.3.0", &[0u8; 64]);
        seed(&root, "templar_registry_contract", "1.1.0", &[0u8; 32]);

        let removed = clean_at(&root).expect("clean succeeds");

        assert_eq!(removed.files, 2);
        assert_eq!(removed.bytes, 96);
        assert!(!root.exists(), "an empty root is removed too");
    }

    #[test]
    fn a_fetch_that_lands_mid_clean_survives_it() {
        let root = scratch_root("concurrent");
        seed(&root, "templar_market_contract", "1.3.0", &[0u8; 64]);

        let writer = std::thread::spawn({
            let root = root.clone();
            move || {
                // A directory that vanished is the clean winning the race.
                let mut lost = 0;
                for version in 0..500 {
                    let dir = root
                        .join("near")
                        .join("templar_registry_contract")
                        .join(version.to_string());
                    if std::fs::create_dir_all(&dir).is_err() {
                        lost += 1;
                        continue;
                    }
                    if let Err(error) = std::fs::write(dir.join("registry.wasm"), [7u8; 8]) {
                        assert_eq!(
                            error.kind(),
                            std::io::ErrorKind::NotFound,
                            "only a concurrent clean may interrupt a cache write"
                        );
                        lost += 1;
                    }
                }
                lost
            }
        });

        let removed = clean_at(&root).expect("clean never fails against a live writer");
        let lost: u32 = writer.join().expect("writer thread never panics");

        assert!(
            !root.join("near/templar_market_contract").exists(),
            "the pre-existing entry was removed"
        );
        // Whatever the rename captured, it can only be what was really there.
        assert!(removed.files <= 501, "reported {} files", removed.files);
        assert!(lost <= 500, "lost {lost} writes");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_cache_variable_is_unset_rather_than_a_relative_path() {
        // `TEMPLAR_ARTIFACT_CACHE=` resolved the root to "", so `clean()` would
        // have deleted `./near` from the working directory.
        assert_eq!(non_empty_path(Some(std::ffi::OsString::from(""))), None);
        assert_eq!(
            non_empty_path(Some(std::ffi::OsString::from("/tmp/cache"))),
            Some(PathBuf::from("/tmp/cache"))
        );
        assert_eq!(non_empty_path(None), None);
    }

    #[test]
    fn cleaning_cannot_reach_outside_the_directory_this_crate_names() {
        // `TEMPLAR_ARTIFACT_CACHE=.` used to delete an unrelated `./near`. The
        // override names the parent, so everything cleanable sits under a
        // directory named after this crate.
        let override_dir = scratch_root("override");
        let victim = override_dir.join("near").join("someone-elses");
        std::fs::create_dir_all(&victim).expect("scratch directory");
        std::fs::write(victim.join("important.txt"), b"not ours").expect("scratch file");

        let root = override_dir.join(CACHE_DIR);
        seed(&root, "templar_market_contract", "1.3.0", &[0u8; 8]);
        let removed = clean_at(&root).expect("clean succeeds");

        assert_eq!(removed.files, 1, "only the cache entry was removed");
        assert!(
            victim.join("important.txt").exists(),
            "a sibling `near/` under the override is not ours to touch"
        );
        let _ = std::fs::remove_dir_all(&override_dir);
    }

    #[test]
    fn cleaning_refuses_a_symlinked_root() {
        let root = scratch_root("symlinked");
        let victim = scratch_root("symlink-victim");
        std::fs::create_dir_all(victim.join("near").join("theirs")).expect("scratch directory");
        std::fs::create_dir_all(root.parent().expect("scratch parent")).ok();
        std::os::unix::fs::symlink(&victim, &root).expect("scratch symlink");

        let error = clean_at(&root).expect_err("a symlinked root redirects the delete");
        assert!(error.to_string().contains("symlink"), "{error}");
        assert!(victim.join("near").join("theirs").exists(), "target intact");

        let _ = std::fs::remove_file(&root);
        let _ = std::fs::remove_dir_all(&victim);
    }

    #[test]
    fn cleaning_an_absent_cache_is_a_no_op() {
        let root = scratch_root("absent");
        let removed = clean_at(&root).expect("clean succeeds");
        assert_eq!(removed, CacheUsage::default());
    }

    #[test]
    fn clean_only_touches_the_subtree_it_owns() {
        // `TEMPLAR_ARTIFACT_CACHE` is arbitrary user input: cleaning must not
        // take unrelated files with it.
        let root = scratch_root("guard");
        seed(&root, "templar_market_contract", "1.3.0", &[0u8; 16]);
        let bystander = root.join("precious.txt");
        std::fs::write(&bystander, b"not ours").expect("bystander file");

        let removed = clean_at(&root).expect("clean succeeds");

        assert_eq!(removed.files, 1, "only the cache entry is counted");
        assert!(bystander.exists(), "unrelated files survive");
        assert!(!root.join("near").exists(), "the owned subtree is gone");

        std::fs::remove_dir_all(&root).expect("scratch cleanup");
    }

    #[test]
    fn only_rate_limiting_and_server_faults_are_retried() {
        assert!(is_retryable(429));
        assert!(is_retryable(500));
        assert!(is_retryable(599));
        for settled in [200, 301, 404, 410, 600] {
            assert!(!is_retryable(settled), "{settled} should not be retried");
        }
    }

    #[test]
    fn asset_url_is_the_recorded_tag_and_asset_under_the_release_base() {
        let release = ArtifactId::ProxyOracle
            .metadata()
            .release("0.3.0")
            .expect("0.3.0 is catalogued");
        assert!(asset_url(release).ends_with(&format!("/{}/{}", release.tag, release.asset)));
    }

    #[tokio::test]
    async fn unknown_release_names_the_catalogued_versions() {
        let error = released_bytes(ArtifactId::ProxyOracle, "9.9.9")
            .await
            .expect_err("9.9.9 was never released");
        let message = error.to_string();
        assert!(message.contains("0.3.0"), "{message}");
    }

    #[tokio::test]
    async fn a_bumped_but_unreleased_version_is_not_fetchable() {
        // proxy-oracle's Cargo.toml is at 0.4.0, but 0.4.0 was never deployed,
        // so it is not a release and there is nothing to fetch.
        let error = released_bytes(ArtifactId::ProxyOracle, "0.4.0")
            .await
            .expect_err("0.4.0 was never released");
        assert!(
            matches!(error, FetchError::UnknownRelease { .. }),
            "{error}"
        );
    }

    #[tokio::test]
    async fn mock_artifacts_have_nothing_to_fetch() {
        let error = released_bytes(ArtifactId::MockFt, "0.0.0")
            .await
            .expect_err("mocks are never released");
        assert!(
            matches!(error, FetchError::UnknownRelease { .. }),
            "{error}"
        );
    }
}
