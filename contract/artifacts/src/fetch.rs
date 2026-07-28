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
//! ${TEMPLAR_ARTIFACT_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/templar-contract-artifacts}
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

/// Root of the shared artifact cache.
pub fn cache_root() -> Result<PathBuf, FetchError> {
    if let Some(explicit) = std::env::var_os("TEMPLAR_ARTIFACT_CACHE") {
        return Ok(PathBuf::from(explicit));
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .ok_or(FetchError::NoCacheDir)?;
    Ok(base.join("templar-contract-artifacts"))
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

    write_atomically(&path, &bytes)?;
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
/// Only the `near/` subtree is removed recursively — the root survives unless
/// that left it empty — so a `TEMPLAR_ARTIFACT_CACHE` pointed at a shared
/// directory cannot take anything else with it.
pub fn clean() -> Result<CacheUsage, FetchError> {
    clean_at(&cache_root()?)
}

/// [`clean`] against an explicit root, so tests need not mutate the environment.
fn clean_at(root: &Path) -> Result<CacheUsage, FetchError> {
    let near = root.join("near");

    let removed = usage_of(&near)?;
    if near.is_dir() {
        std::fs::remove_dir_all(&near).map_err(|source| FetchError::Cache {
            path: near.clone(),
            source,
        })?;
    }
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
