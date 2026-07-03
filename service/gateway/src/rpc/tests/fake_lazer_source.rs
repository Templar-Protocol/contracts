//! Deterministic in-memory stand-in for the Pyth Lazer payload source.
//!
//! The real `LazerPayloadSource` maintains a websocket cache and cannot run in a
//! unit test without a live Lazer endpoint. This fake implements the same
//! `OraclePayloadSource<PriceId = u32>` boundary and returns a canned payload or
//! a controlled error, so RPC-level Lazer tests are fully deterministic (no
//! network, no timing, no `near-workspaces` dependency for the plan-only cases).
//!
//! `WithFakeLazerSource<C>` mirrors the production `WithLazerSource<C>` wrapper
//! but holds the fake source type instead of the concrete `LazerPayloadSource`,
//! so the test context can satisfy the `ProvidesLazerSource` bound without
//! spawning a websocket task.

use std::sync::Arc;

use async_trait::async_trait;
use templar_gateway_core::{HasNearClient, NearClient, OraclePayloadSource};
use templar_gateway_oracle_updates_dispatch::{
    ProvidesLazerSource, ProvidesPythSource, ProvidesRedStoneSource,
};
use thiserror::Error;

/// The controlled outcomes the fake Lazer source can produce. The variant
/// messages mirror the real `LazerClientError` diagnostics so the resulting
/// `GatewayError::ExternalService` messages stay realistic and assertable.
#[derive(Debug, Clone, Error)]
pub enum FakeLazerError {
    #[error("Pyth Lazer cache miss: no payload available")]
    CacheMiss,
    #[error("Pyth Lazer stale payload: cached value exceeded max age")]
    Stale,
}

/// The canned result the fake source returns for every `fetch_payload` call.
#[derive(Debug, Clone)]
pub struct FakeLazerOutcome(pub Result<Arc<[u8]>, FakeLazerError>);

impl FakeLazerOutcome {
    pub fn ok(bytes: impl Into<Vec<u8>>) -> Self {
        Self(Ok(bytes.into().into_boxed_slice().into()))
    }

    pub fn err(error: FakeLazerError) -> Self {
        Self(Err(error))
    }
}

/// Cloneable fake `OraclePayloadSource<PriceId = u32>`. Every call returns the
/// same canned outcome, so tests are deterministic regardless of feed id or
/// call order.
#[derive(Debug, Clone)]
pub struct FakeLazerSource {
    outcome: FakeLazerOutcome,
}

impl FakeLazerSource {
    pub fn new(outcome: FakeLazerOutcome) -> Self {
        Self { outcome }
    }

    /// Convenience: a source that always succeeds with the given payload bytes.
    pub fn with_payload(bytes: impl Into<Vec<u8>>) -> Self {
        Self::new(FakeLazerOutcome::ok(bytes))
    }

    /// Convenience: a source that always fails with the given controlled error.
    pub fn failing(error: FakeLazerError) -> Self {
        Self::new(FakeLazerOutcome::err(error))
    }
}

#[async_trait]
impl OraclePayloadSource for FakeLazerSource {
    type PriceId = u32;
    type Error = FakeLazerError;

    async fn fetch_payload(&self, _price_ids: &[u32]) -> Result<Vec<u8>, FakeLazerError> {
        // Cloning the canned result keeps the source reusable across calls and
        // keeps tests deterministic: no shared mutable state, no call ordering.
        self.outcome.0.clone().map(|bytes| bytes.to_vec())
    }
}

/// Test context wrapper that injects a [`FakeLazerSource`] while delegating the
/// near client and any outer Pyth/RedStone sources to the inner context. This
/// is the test-only analogue of the production `WithLazerSource<C>` wrapper,
/// generalized over the source type so a fake can stand in.
#[derive(Debug, Clone)]
pub struct WithFakeLazerSource<C> {
    inner: C,
    lazer_source: FakeLazerSource,
}

impl<C> WithFakeLazerSource<C> {
    pub fn new(inner: C, lazer_source: FakeLazerSource) -> Self {
        Self {
            inner,
            lazer_source,
        }
    }
}

impl<C: HasNearClient> HasNearClient for WithFakeLazerSource<C> {
    fn near_client(&self) -> &NearClient {
        self.inner.near_client()
    }
}

impl<C: ProvidesPythSource> ProvidesPythSource for WithFakeLazerSource<C> {
    type PythSource = C::PythSource;

    fn pyth_source(&self) -> &Self::PythSource {
        self.inner.pyth_source()
    }
}

impl<C: ProvidesRedStoneSource> ProvidesRedStoneSource for WithFakeLazerSource<C> {
    type RedStoneSource = C::RedStoneSource;

    fn redstone_source(&self) -> &Self::RedStoneSource {
        self.inner.redstone_source()
    }
}

impl<C> ProvidesLazerSource for WithFakeLazerSource<C> {
    type LazerSource = FakeLazerSource;

    fn lazer_source(&self) -> &FakeLazerSource {
        &self.lazer_source
    }
}
