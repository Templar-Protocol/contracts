//! Download and cache released contract WASM.
//!
//! Enabled via the `fetch` feature.
//!
//! # Where the bytes live
//!
//! Released bytes are GitHub Release assets, not repository content. Each
//! release's tag (`{package}-v{version}`) carries one asset,
//! `{cargo_target_name}-{version}.wasm`, built by
//! `.github/workflows/release-artifacts.yml` in the pinned NEP-330 Docker image
//! **at that tag's own commit**.
//!
//! Building at the tag is the whole point. `cargo near build reproducible-wasm`
//! embeds its source commit into the WASM, so a rebuild only matches at the
//! commit it was built from — and that commit has to be permanently reachable,
//! or nobody (including nearblocks.io) can verify a deployed contract against
//! this source tree. A tag is reachable from a fresh `git clone` forever; a
//! feature branch that was squash-merged is not.
//!
//! # Trust
//!
//! Downloaded bytes are verified against the SHA-256 pinned in the catalog
//! ([`ArtifactRelease::sha256`]) and discarded on mismatch. That pin is a
//! reviewed, in-repo value, so artifact integrity does not rest on GitHub
//! serving us the right file.
//!
//! # Cache
//!
//! Bytes land in a shared cache outside the repository, so every worktree and
//! every test binary reuses one copy:
//!
//! ```text
//! ${TEMPLAR_ARTIFACT_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/templar-contract-artifacts}
//!   └── near/<cargo_target_name>/<version>/<cargo_target_name>.wasm
//! ```
//!
//! `TEMPLAR_ARTIFACT_OFFLINE=1` restricts lookups to the cache. Warm it with
//! `just artifacts-fetch`.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use thiserror::Error;

use crate::{sha256_hex, ArtifactId};

/// Repository whose GitHub Releases host the artifact assets.
const RELEASE_REPO: &str = "Templar-Protocol/contracts";

/// Download attempts before giving up. Assets are immutable, so a retry can
/// only ever fetch the same bytes.
const DOWNLOAD_ATTEMPTS: u32 = 3;

const RETRY_BACKOFF: Duration = Duration::from_millis(500);

/// Simultaneous downloads during a prefetch. Enough to hide per-request
/// latency, low enough to stay well clear of GitHub's rate limiting.
const PREFETCH_CONCURRENCY: usize = 8;

#[derive(Debug, Error)]
pub enum FetchError {
    #[error(
        "{artifact} has no release {version}; catalogued releases: [{catalogued}]. \
         Historical versions live in contract/artifacts/src/ids.rs."
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

/// The tag that carries this release: matches `git_tag_name` in `release-plz.toml`.
pub fn release_tag(artifact: ArtifactId, version: &str) -> String {
    format!("{}-v{version}", artifact.metadata().package_name)
}

/// Download URL for a release's WASM asset.
pub fn asset_url(artifact: ArtifactId, version: &str) -> String {
    let base = std::env::var("TEMPLAR_ARTIFACT_BASE_URL")
        .unwrap_or_else(|_| format!("https://github.com/{RELEASE_REPO}/releases/download"));
    let target = artifact.metadata().cargo_target_name;
    format!(
        "{base}/{tag}/{target}-{version}.wasm",
        tag = release_tag(artifact, version),
    )
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
                .releases
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

    let url = asset_url(artifact, version);
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

/// Populate the cache with every release in the catalog.
///
/// Returns the number of releases now cached.
///
/// Downloads run concurrently: the whole catalog is only a few MB, so wall
/// clock here is dominated by per-request latency to GitHub's CDN, not
/// bandwidth. Sequentially this takes minutes, which CI pays on every run.
pub async fn prefetch_all() -> Result<usize, FetchError> {
    let releases = ArtifactId::ALL
        .into_iter()
        .flat_map(|artifact| {
            artifact
                .metadata()
                .releases
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

async fn download(url: &str) -> Result<Vec<u8>, FetchError> {
    let mut last = None;
    for attempt in 0..DOWNLOAD_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(RETRY_BACKOFF * 2u32.pow(attempt - 1)).await;
        }
        match try_download(url).await {
            Ok(bytes) => return Ok(bytes),
            // A 404 is not transient: the asset genuinely is not there.
            Err(error @ FetchError::Status { status: 404, .. }) => return Err(error),
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
        // Unreachable: the loop only exits here after recording a transport
        // error, but a panic would be a worse failure mode than a plain status.
        None => FetchError::Status {
            url: url.to_owned(),
            status: 0,
        },
    })
}

async fn try_download(url: &str) -> Result<Vec<u8>, FetchError> {
    let response = reqwest::get(url)
        .await
        .map_err(|source| FetchError::Download {
            url: url.to_owned(),
            attempts: 1,
            source,
        })?;

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
        .map_err(|source| FetchError::Download {
            url: url.to_owned(),
            attempts: 1,
            source,
        })
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

    #[test]
    fn asset_url_matches_the_release_tag_and_uploaded_asset_name() {
        // Tag shape mirrors `git_tag_name` in release-plz.toml; asset shape
        // mirrors what release-artifacts.yml uploads.
        assert_eq!(
            release_tag(ArtifactId::ProxyOracle, "0.3.0"),
            "templar-proxy-oracle-near-contract-v0.3.0",
        );
        assert!(asset_url(ArtifactId::ProxyOracle, "0.3.0").ends_with(
            "/templar-proxy-oracle-near-contract-v0.3.0/templar_proxy_oracle_near_contract-0.3.0.wasm"
        ));
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
