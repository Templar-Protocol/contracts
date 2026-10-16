use std::collections::{BTreeMap, BTreeSet, HashMap};

use async_trait::async_trait;
use near_account_id::AccountId;
use templar_common::{
    oracle::{lazer, pyth::PriceIdentifier, redstone},
    Nanoseconds,
};
use templar_gateway_core::{
    client::{
        proxy_oracle::UpdatePricesArgs, pyth_lazer_oracle::UpdatePriceFeedsArgs,
        redstone_oracle::WritePricesArgs, ContractWriteOptions,
    },
    plan_pyth_lazer_update, plan_pyth_update, plan_redstone_write_prices, query_oracle_kind,
    resolve_price_dependencies, DispatchRead, GatewayError, GatewayResult, HasNearClient,
    OperationPlan, OraclePayloadSource, PlanWrite, PlannedTransaction,
};
use templar_gateway_oracle_updates_spec::oracle::{
    GetLazerUpdate, GetRedStoneUpdate, UpdateLazer, UpdatePrices, UpdatePyth, UpdateRedStone,
};
use templar_gateway_types::{ManagedAccountId, OracleContractKind};
use templar_proxy_oracle_near_common::request::{LazerRequest, OracleRequest};

use crate::{Dispatch, ProvidesLazerSource, ProvidesPythSource, ProvidesRedStoneSource};

#[async_trait]
impl<C: HasNearClient + ProvidesLazerSource> DispatchRead<GetLazerUpdate, C> for Dispatch {
    async fn dispatch(
        request: GetLazerUpdate,
        ctx: C,
    ) -> GatewayResult<HashMap<u32, Option<lazer::FeedData>>> {
        if request.feed_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let payload = ctx
            .lazer_source()
            .fetch_payload(&request.feed_ids)
            .await
            .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
        let snapshot = ctx.near_client().chain().block(None).await?;
        let oracle = ctx.near_client().pyth_lazer_oracle(request.oracle_id);
        let config = oracle.get_projection_config_at(snapshot.hash).await?;
        let verified = oracle
            .verify_update_at(
                UpdatePriceFeedsArgs {
                    payload: near_sdk::json_types::Base64VecU8(payload),
                },
                snapshot.hash,
            )
            .await?;
        let now = Nanoseconds::from_ns(snapshot.timestamp_ns);
        let mut result: HashMap<u32, Option<lazer::FeedData>> =
            request.feed_ids.into_iter().map(|id| (id, None)).collect();
        for parsed in &verified.feeds {
            let Some(selected) = result.get_mut(&parsed.feed_id) else {
                continue;
            };
            let Some(candidate) = lazer::feed_data_from_parsed(
                parsed,
                verified.timestamp_ns,
                now,
                config.max_timestamp_ahead_s,
            ) else {
                continue;
            };
            // Match the adapter's within-bundle monotonic selection, not stored-state replay.
            if selected
                .as_ref()
                .is_none_or(|old| candidate.publish_time_ns > old.publish_time_ns)
            {
                *selected = Some(candidate);
            }
        }
        Ok(result)
    }
}

#[async_trait]
impl<C: HasNearClient + ProvidesRedStoneSource> DispatchRead<GetRedStoneUpdate, C> for Dispatch {
    async fn dispatch(
        request: GetRedStoneUpdate,
        ctx: C,
    ) -> GatewayResult<Vec<templar_gateway_methods_spec::redstone::PriceDataEntry>> {
        if request.feed_ids.is_empty() {
            return Ok(Vec::new());
        }
        let payload = ctx
            .redstone_source()
            .fetch_payload(&request.feed_ids)
            .await
            .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
        let snapshot = ctx.near_client().chain().block(None).await?;
        // The verifier requires unique IDs; preserve the original request for output ordering.
        let mut verification_feed_ids = request.feed_ids.clone();
        verification_feed_ids.sort_unstable();
        verification_feed_ids.dedup();
        let verified = ctx
            .near_client()
            .redstone_oracle(request.oracle_id)
            .get_prices_at(
                WritePricesArgs {
                    feed_ids: verification_feed_ids,
                    payload: near_sdk::json_types::Base64VecU8(payload),
                },
                snapshot.hash,
            )
            .await?;
        request
            .feed_ids
            .into_iter()
            .map(|feed_id| {
                // The adapter omits missing feeds rather than rejecting them.
                // A provider preview must not silently return an incomplete request.
                let price = verified.prices.get(&feed_id).copied().ok_or_else(|| {
                    GatewayError::ExternalService(format!(
                        "verified RedStone payload is missing requested feed {feed_id}"
                    ))
                })?;
                Ok(templar_gateway_methods_spec::redstone::PriceDataEntry {
                    feed_id,
                    data: redstone::FeedData {
                        price,
                        package_timestamp: verified.timestamp,
                        write_timestamp: Nanoseconds::from_ns(snapshot.timestamp_ns),
                    },
                })
            })
            .collect()
    }
}

#[async_trait]
impl<C> PlanWrite<UpdatePyth, C> for Dispatch
where
    C: HasNearClient + ProvidesPythSource,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<UpdatePyth>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        if body.price_ids.is_empty() {
            tracing::warn!(
                oracle_id = %body.oracle_id,
                "oracle.updatePyth requested with no price ids; nothing to update"
            );
            return Ok(OperationPlan { steps: Vec::new() });
        }
        plan_pyth_feed_update(
            &ctx,
            request.signer_account_id,
            body.oracle_id,
            &body.price_ids,
        )
        .await
        .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C> PlanWrite<UpdateRedStone, C> for Dispatch
where
    C: HasNearClient + ProvidesRedStoneSource,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<UpdateRedStone>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        // Nothing to fetch and nothing to write. A step-less plan settles as a terminal
        // no-op, matching `oracle.updatePrices` with no price ids.
        if body.feed_ids.is_empty() {
            tracing::warn!(
                oracle_id = %body.oracle_id,
                "oracle.updateRedStone requested with no feed ids; nothing to update"
            );
            return Ok(OperationPlan { steps: Vec::new() });
        }
        tracing::debug!(
            oracle_id = %body.oracle_id,
            feed_count = body.feed_ids.len(),
            "fetching RedStone payload for gateway oracle update"
        );
        let payload = OraclePayloadSource::fetch_payload(ctx.redstone_source(), &body.feed_ids)
            .await
            .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
        plan_redstone_write_prices(
            ctx.near_client(),
            request.signer_account_id,
            body.oracle_id,
            body.feed_ids,
            payload,
        )
        .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C> PlanWrite<UpdateLazer, C> for Dispatch
where
    C: HasNearClient + ProvidesLazerSource,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<UpdateLazer>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        if body.feed_ids.is_empty() {
            tracing::warn!(
                oracle_id = %body.oracle_id,
                "oracle.updateLazer requested with no feed ids; nothing to update"
            );
            return Ok(OperationPlan { steps: Vec::new() });
        }
        plan_lazer_feed_update(
            &ctx,
            request.signer_account_id,
            body.oracle_id,
            &body.feed_ids,
        )
        .await
        .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C> PlanWrite<UpdatePrices, C> for Dispatch
where
    C: HasNearClient + ProvidesPythSource + ProvidesRedStoneSource + ProvidesLazerSource,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<UpdatePrices>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let signer_account_id = request.signer_account_id;
        let oracle_id = request.body.oracle_id;
        let price_ids = request.body.price_ids;

        let (kind, requests) = resolve_update_requests(&ctx, oracle_id.clone(), &price_ids).await?;

        let mut plan = plan_grouped_updates(&ctx, signer_account_id.clone(), requests).await?;

        // When the target is a proxy oracle, re-aggregate its cached prices after the
        // underlying updates. Steps execute sequentially, so the proxy read sees the fresh
        // underlying prices this same operation just wrote.
        if matches!(kind, OracleContractKind::Proxy) {
            // The underlying oracle updates are mutually independent: one reverting must
            // not cancel the others, nor the re-aggregation. Mark them continue_on_failure
            // so a revert is recorded and tolerated while the operation advances. The
            // re-aggregation step appended below stays non-fallible — it always runs (after
            // whatever underlying prices did land) and its outcome is the operation's verdict.
            // A direct/LST oracle takes neither branch, so its single underlying update stays
            // all-or-nothing.
            for step in &mut plan.steps {
                step.continue_on_failure = true;
            }
            plan.steps
                .push(ctx.near_client().proxy_oracle(oracle_id).update_prices(
                    ContractWriteOptions::new(signer_account_id).tgas(100),
                    UpdatePricesArgs { price_ids },
                )?);
        }

        Ok(plan)
    }
}

/// Fetch the VAA covering `price_ids` from Hermes and plan the adapter write.
async fn plan_pyth_feed_update<C>(
    ctx: &C,
    signer_account_id: ManagedAccountId,
    oracle_id: AccountId,
    price_ids: &[PriceIdentifier],
) -> GatewayResult<PlannedTransaction>
where
    C: HasNearClient + ProvidesPythSource,
{
    tracing::debug!(
        %oracle_id,
        price_count = price_ids.len(),
        "fetching Pyth payload for gateway oracle update"
    );
    let vaa = OraclePayloadSource::fetch_payload(ctx.pyth_source(), price_ids)
        .await
        .map_err(|error| GatewayError::HttpRequest(error.to_string()))?;
    plan_pyth_update(ctx.near_client(), signer_account_id, oracle_id, vaa)
}

/// Fetch the payload covering `feed_ids` from the Lazer stream and plan the adapter write.
async fn plan_lazer_feed_update<C>(
    ctx: &C,
    signer_account_id: ManagedAccountId,
    oracle_id: AccountId,
    feed_ids: &[u32],
) -> GatewayResult<PlannedTransaction>
where
    C: HasNearClient + ProvidesLazerSource,
{
    tracing::debug!(
        %oracle_id,
        feed_count = feed_ids.len(),
        "fetching Pyth Lazer payload for gateway oracle update"
    );
    let payload = OraclePayloadSource::fetch_payload(ctx.lazer_source(), feed_ids)
        .await
        .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
    plan_pyth_lazer_update(ctx.near_client(), signer_account_id, oracle_id, payload)
}

async fn plan_grouped_updates<C>(
    ctx: &C,
    signer_account_id: ManagedAccountId,
    requests: Vec<OracleRequest>,
) -> GatewayResult<OperationPlan>
where
    C: HasNearClient + ProvidesPythSource + ProvidesRedStoneSource + ProvidesLazerSource,
{
    let mut steps = Vec::new();
    let mut pyth_updates = BTreeMap::<AccountId, BTreeSet<PriceIdentifier>>::new();
    let mut redstone_updates = BTreeMap::<AccountId, BTreeSet<redstone::FeedId>>::new();
    let mut lazer_updates = BTreeMap::<AccountId, BTreeSet<u32>>::new();

    for request in requests {
        match request {
            OracleRequest::Pyth(request) => {
                pyth_updates
                    .entry(request.oracle_id)
                    .or_default()
                    .insert(request.price_id);
            }
            OracleRequest::RedStone(request) => {
                redstone_updates
                    .entry(request.oracle_id)
                    .or_default()
                    .insert(request.price_id);
            }
            OracleRequest::Lazer(LazerRequest { oracle_id, feed_id }) => {
                lazer_updates.entry(oracle_id).or_default().insert(feed_id);
            }
        }
    }

    tracing::debug!(
        pyth_oracle_count = pyth_updates.len(),
        redstone_oracle_count = redstone_updates.len(),
        lazer_oracle_count = lazer_updates.len(),
        "resolved oracle update dependencies"
    );

    for (oracle_id, price_ids) in pyth_updates {
        let price_ids = price_ids.into_iter().collect::<Vec<_>>();
        steps.push(
            plan_pyth_feed_update(ctx, signer_account_id.clone(), oracle_id, &price_ids).await?,
        );
    }

    for (oracle_id, feed_ids) in redstone_updates {
        let feed_ids = feed_ids.into_iter().collect::<Vec<_>>();
        tracing::debug!(
            %oracle_id,
            feed_count = feed_ids.len(),
            "fetching RedStone payload for gateway oracle update"
        );
        let payload = OraclePayloadSource::fetch_payload(ctx.redstone_source(), &feed_ids)
            .await
            .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
        steps.push(plan_redstone_write_prices(
            ctx.near_client(),
            signer_account_id.clone(),
            oracle_id,
            feed_ids,
            payload,
        )?);
    }

    for (oracle_id, feed_ids) in lazer_updates {
        let feed_ids: Vec<u32> = feed_ids.into_iter().collect();
        steps.push(
            plan_lazer_feed_update(ctx, signer_account_id.clone(), oracle_id, &feed_ids).await?,
        );
    }

    Ok(OperationPlan { steps })
}

/// Resolve every requested `price_id` on `oracle_id` into the underlying source updates,
/// returning the oracle's kind alongside so the caller can append the proxy re-aggregation
/// step. Delegates to the shared `gateway_core` resolution so writes agree with reads and
/// `oracle.getPriceResolutionDependencies` on how each oracle resolves.
async fn resolve_update_requests<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
    price_ids: &[PriceIdentifier],
) -> GatewayResult<(OracleContractKind, Vec<OracleRequest>)> {
    let kind = query_oracle_kind(ctx, oracle_id.clone()).await?;
    let mut requests = BTreeSet::new();

    for &price_id in price_ids {
        requests.extend(resolve_price_dependencies(ctx, oracle_id.clone(), price_id, &kind).await?);
    }

    Ok((kind, requests.into_iter().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use near_api::types::transaction::actions::Action;
    use near_api::NetworkConfig;
    use rstest::rstest;
    use std::sync::{Arc, Mutex};
    use templar_gateway_core::NearClient;
    use templar_gateway_types::common::WriteRequest;
    use thiserror::Error;

    // Every source error carries `err.to_string()` into its gateway variant, so one
    // variant exercises that path; the tests differ by entry point, not error kind.
    #[derive(Debug, Clone, Error)]
    #[error("fake source unavailable")]
    struct FakeError;

    #[derive(Clone)]
    struct FakeLazerSource {
        outcome: Result<Vec<u8>, FakeError>,
        calls: Arc<Mutex<Vec<Vec<u32>>>>,
    }

    impl FakeLazerSource {
        fn new(outcome: Result<Vec<u8>, FakeError>) -> Self {
            Self {
                outcome,
                calls: Arc::default(),
            }
        }

        fn calls(&self) -> Vec<Vec<u32>> {
            self.calls
                .lock()
                .expect("calls mutex must not be poisoned")
                .clone()
        }
    }

    #[async_trait]
    impl OraclePayloadSource for FakeLazerSource {
        type PriceId = u32;
        type Error = FakeError;

        async fn fetch_payload(&self, price_ids: &[u32]) -> Result<Vec<u8>, FakeError> {
            self.calls
                .lock()
                .expect("calls mutex must not be poisoned")
                .push(price_ids.to_vec());
            self.outcome.clone()
        }
    }

    #[derive(Clone)]
    struct FakePythSource {
        outcome: Result<Vec<u8>, FakeError>,
    }

    #[async_trait]
    impl OraclePayloadSource for FakePythSource {
        type PriceId = PriceIdentifier;
        type Error = FakeError;

        async fn fetch_payload(
            &self,
            _price_ids: &[PriceIdentifier],
        ) -> Result<Vec<u8>, FakeError> {
            self.outcome.clone()
        }
    }

    #[derive(Clone)]
    struct FakeRedStoneSource {
        outcome: Result<Vec<u8>, FakeError>,
    }

    #[async_trait]
    impl OraclePayloadSource for FakeRedStoneSource {
        type PriceId = redstone::FeedId;
        type Error = FakeError;

        async fn fetch_payload(
            &self,
            _feed_ids: &[redstone::FeedId],
        ) -> Result<Vec<u8>, FakeError> {
            self.outcome.clone()
        }
    }

    #[derive(Clone)]
    struct TestCtx {
        near_client: NearClient,
        pyth_source: FakePythSource,
        redstone_source: FakeRedStoneSource,
        lazer_source: FakeLazerSource,
    }

    impl HasNearClient for TestCtx {
        fn near_client(&self) -> &NearClient {
            &self.near_client
        }
    }

    impl ProvidesPythSource for TestCtx {
        type PythSource = FakePythSource;

        fn pyth_source(&self) -> &FakePythSource {
            &self.pyth_source
        }
    }

    impl ProvidesRedStoneSource for TestCtx {
        type RedStoneSource = FakeRedStoneSource;

        fn redstone_source(&self) -> &FakeRedStoneSource {
            &self.redstone_source
        }
    }

    impl ProvidesLazerSource for TestCtx {
        type LazerSource = FakeLazerSource;

        fn lazer_source(&self) -> &FakeLazerSource {
            &self.lazer_source
        }
    }

    fn test_client() -> NearClient {
        NearClient::new(NetworkConfig::from_rpc_url(
            "test",
            "https://example.test".parse().expect("valid url"),
        ))
    }

    fn signer_id() -> ManagedAccountId {
        ManagedAccountId("relayer.near".parse().expect("valid account id"))
    }

    fn unpack_function_call(step: &PlannedTransaction) -> (String, Vec<u8>) {
        assert_eq!(
            step.actions.len(),
            1,
            "planner must produce exactly one action, got {}",
            step.actions.len()
        );
        match &step.actions[0] {
            Action::FunctionCall(fc) => (fc.method_name.clone(), fc.args.clone()),
            other => panic!("expected FunctionCall action, got {other:?}"),
        }
    }

    /// A `TestCtx` whose every source succeeds with an empty payload. Override the one
    /// source under test with struct-update syntax.
    fn inert_ctx() -> TestCtx {
        TestCtx {
            near_client: test_client(),
            pyth_source: FakePythSource {
                outcome: Ok(Vec::new()),
            },
            redstone_source: FakeRedStoneSource {
                outcome: Ok(Vec::new()),
            },
            lazer_source: FakeLazerSource::new(Ok(Vec::new())),
        }
    }

    fn pyth_ctx(outcome: Result<Vec<u8>, FakeError>) -> TestCtx {
        TestCtx {
            pyth_source: FakePythSource { outcome },
            ..inert_ctx()
        }
    }

    fn lazer_ctx(outcome: Result<Vec<u8>, FakeError>) -> TestCtx {
        TestCtx {
            lazer_source: FakeLazerSource::new(outcome),
            ..inert_ctx()
        }
    }

    fn write_request<B>(body: B) -> WriteRequest<B> {
        WriteRequest {
            signer_account_id: signer_id(),
            idempotency_key: None,
            body,
        }
    }

    #[tokio::test]
    async fn update_pyth_writes_the_vaa_it_fetched() {
        let vaa = vec![0x01, 0x02, 0x03, 0x04];
        let oracle_id: AccountId = "pyth.near".parse().expect("valid account id");
        let request = write_request(UpdatePyth {
            oracle_id: oracle_id.clone(),
            price_ids: vec![PriceIdentifier([0xAA; 32])],
        });

        let plan =
            <Dispatch as PlanWrite<UpdatePyth, TestCtx>>::plan(request, pyth_ctx(Ok(vaa.clone())))
                .await
                .expect("UpdatePyth plan must succeed with a fresh payload");

        assert_eq!(plan.steps.len(), 1, "UpdatePyth must plan exactly one step");
        let step = &plan.steps[0];
        assert_eq!(step.receiver_id, oracle_id);

        let (method, args_bytes) = unpack_function_call(step);
        assert_eq!(method, "update_price_feeds");
        let args_json: serde_json::Value =
            serde_json::from_slice(&args_bytes).expect("args must be valid json");
        assert_eq!(
            args_json["data"],
            hex::encode(&vaa),
            "the adapter must receive the fetched VAA as hex; got: {args_json}"
        );
    }

    #[tokio::test]
    async fn update_pyth_with_no_price_ids_plans_nothing() {
        let request = write_request(UpdatePyth {
            oracle_id: "pyth.near".parse().unwrap(),
            price_ids: Vec::new(),
        });

        let plan =
            <Dispatch as PlanWrite<UpdatePyth, TestCtx>>::plan(request, pyth_ctx(Err(FakeError)))
                .await
                .expect("an empty UpdatePyth must not reach the source");

        assert!(plan.steps.is_empty(), "expected a step-less plan");
    }

    #[tokio::test]
    async fn update_pyth_propagates_source_error() {
        let request = write_request(UpdatePyth {
            oracle_id: "pyth.near".parse().unwrap(),
            price_ids: vec![PriceIdentifier([0xAA; 32])],
        });

        let error =
            <Dispatch as PlanWrite<UpdatePyth, TestCtx>>::plan(request, pyth_ctx(Err(FakeError)))
                .await
                .expect_err("a Hermes error must surface as a plan error");

        assert!(
            matches!(error, GatewayError::HttpRequest(ref msg) if msg.contains("unavailable")),
            "expected HttpRequest carrying the source-error detail, got {error:?}"
        );
    }

    #[rstest]
    #[case::single_feed(vec![7])]
    #[case::multiple_feeds(vec![7, 8])]
    #[tokio::test]
    async fn update_lazer_plans_one_adapter_write(#[case] feed_ids: Vec<u32>) {
        let payload = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let ctx = lazer_ctx(Ok(payload.clone()));
        // Shares the recorder with the context `plan` consumes.
        let source = ctx.lazer_source.clone();
        let oracle_id: AccountId = "pyth-lazer.near".parse().expect("valid account id");
        let request = write_request(UpdateLazer {
            oracle_id: oracle_id.clone(),
            feed_ids: feed_ids.clone(),
        });

        let plan = <Dispatch as PlanWrite<UpdateLazer, TestCtx>>::plan(request, ctx)
            .await
            .expect("UpdateLazer plan must succeed with a fresh payload");

        assert_eq!(
            source.calls(),
            vec![feed_ids],
            "every requested feed must reach the source in a single fetch"
        );
        assert_eq!(
            plan.steps.len(),
            1,
            "UpdateLazer must plan exactly one step for the whole feed set"
        );
        let step = &plan.steps[0];
        assert_eq!(step.receiver_id, oracle_id);

        let (method, args_bytes) = unpack_function_call(step);
        assert_eq!(method, "update_price_feeds");

        let args_json: serde_json::Value =
            serde_json::from_slice(&args_bytes).expect("args must be valid json");
        assert!(
            args_json.get("payload").is_some(),
            "UpdateLazer args MUST carry `payload` (base64); got: {args_json}"
        );
        assert!(
            args_json.get("data").is_none(),
            "UpdateLazer args MUST NOT carry `data`; got: {args_json}"
        );

        let payload_b64 = args_json["payload"]
            .as_str()
            .expect("`payload` must be a json string");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(payload_b64)
            .expect("payload must be valid base64");
        assert_eq!(
            decoded, payload,
            "payload must round-trip to the fetched bytes"
        );
    }

    #[tokio::test]
    async fn update_lazer_with_no_feed_ids_plans_nothing() {
        let ctx = lazer_ctx(Err(FakeError));
        let request = write_request(UpdateLazer {
            oracle_id: "pyth-lazer.near".parse().unwrap(),
            feed_ids: Vec::new(),
        });

        let plan = <Dispatch as PlanWrite<UpdateLazer, TestCtx>>::plan(request, ctx)
            .await
            .expect("an empty UpdateLazer must not reach the source");

        assert!(plan.steps.is_empty(), "expected a step-less plan");
    }

    #[tokio::test]
    async fn update_lazer_propagates_source_error() {
        let ctx = lazer_ctx(Err(FakeError));
        let request = write_request(UpdateLazer {
            oracle_id: "pyth-lazer.near".parse().unwrap(),
            feed_ids: vec![7],
        });

        let error = <Dispatch as PlanWrite<UpdateLazer, TestCtx>>::plan(request, ctx)
            .await
            .expect_err("a Lazer source error must surface as a plan error");

        assert!(
            matches!(error, GatewayError::ExternalService(ref msg) if msg.contains("unavailable")),
            "expected ExternalService carrying the source-error detail, got {error:?}"
        );
    }

    #[tokio::test]
    async fn update_prices_groups_lazer_separately() {
        let pyth_payload = vec![0x11_u8; 16];
        let lazer_payload = vec![0x22_u8; 24];
        let ctx = TestCtx {
            pyth_source: FakePythSource {
                outcome: Ok(pyth_payload.clone()),
            },
            lazer_source: FakeLazerSource::new(Ok(lazer_payload.clone())),
            ..inert_ctx()
        };
        let pyth_oracle: AccountId = "pyth.near".parse().unwrap();
        let lazer_oracle: AccountId = "pyth-lazer.near".parse().unwrap();
        let price_id = PriceIdentifier([0xAA; 32]);

        let requests = vec![
            OracleRequest::pyth(pyth_oracle.clone(), price_id),
            OracleRequest::lazer(lazer_oracle.clone(), 42),
        ];

        let plan = plan_grouped_updates(&ctx, signer_id(), requests)
            .await
            .expect("mixed Pyth + Lazer grouping must succeed");

        assert_eq!(
            plan.steps.len(),
            2,
            "mixed grouping must produce one step per oracle"
        );
        assert!(
            plan.steps.iter().all(|step| !step.continue_on_failure),
            "plan_grouped_updates must leave steps all-or-nothing; marking underlying \
             updates continue_on_failure is the proxy branch's job in UpdatePrices::plan"
        );

        let mut found_pyth_data_step = false;
        let mut found_lazer_payload_step = false;
        for step in &plan.steps {
            let (method, args_bytes) = unpack_function_call(step);
            assert_eq!(method, "update_price_feeds");
            let args_json: serde_json::Value =
                serde_json::from_slice(&args_bytes).expect("step args must be valid json");

            if step.receiver_id == pyth_oracle {
                assert!(
                    args_json.get("data").is_some(),
                    "classic Pyth step MUST carry `data` (hex); got: {args_json}"
                );
                assert!(
                    args_json.get("payload").is_none(),
                    "classic Pyth step MUST NOT carry `payload`; got: {args_json}"
                );
                found_pyth_data_step = true;
            } else if step.receiver_id == lazer_oracle {
                assert!(
                    args_json.get("payload").is_some(),
                    "Lazer step MUST carry `payload` (base64); got: {args_json}"
                );
                assert!(
                    args_json.get("data").is_none(),
                    "Lazer step MUST NOT carry `data`; got: {args_json}"
                );
                found_lazer_payload_step = true;
            }
        }
        assert!(
            found_pyth_data_step,
            "mixed plan must include a classic Pyth `data` step"
        );
        assert!(
            found_lazer_payload_step,
            "mixed plan must include a Lazer `payload` step"
        );
    }

    #[tokio::test]
    async fn update_prices_lazer_source_error_is_hard_error() {
        let ctx = lazer_ctx(Err(FakeError));
        let lazer_oracle: AccountId = "pyth-lazer.near".parse().unwrap();
        let requests = vec![OracleRequest::lazer(lazer_oracle, 7)];

        let error = plan_grouped_updates(&ctx, signer_id(), requests)
            .await
            .expect_err("a Lazer source error must be a hard plan error");

        assert!(
            matches!(error, GatewayError::ExternalService(ref msg) if msg.contains("unavailable")),
            "expected ExternalService carrying the source-error detail, got {error:?}"
        );
    }

    // The proxy re-aggregation step `UpdatePrices::plan` appends for a proxy oracle must
    // call the proxy's own `update_prices` with the requested (proxy-level) price ids. The
    // kind gating itself needs a live `contract.getKind` query and is covered by the
    // gateway sandbox test `oracle_update_prices_endpoint_resolves_and_updates_dependencies`.
    #[test]
    fn proxy_reaggregation_step_calls_proxy_update_prices() {
        let ctx = inert_ctx();
        let oracle_id: AccountId = "proxy.near".parse().expect("valid account id");
        let price_ids = vec![PriceIdentifier([0x11; 32]), PriceIdentifier([0x22; 32])];

        let step = ctx
            .near_client()
            .proxy_oracle(oracle_id.clone())
            .update_prices(
                ContractWriteOptions::new(signer_id()).tgas(100),
                UpdatePricesArgs {
                    price_ids: price_ids.clone(),
                },
            )
            .expect("proxy re-aggregation step must build");

        assert_eq!(step.receiver_id, oracle_id);
        let (method, args_bytes) = unpack_function_call(&step);
        assert_eq!(method, "update_prices");
        let args_json: serde_json::Value =
            serde_json::from_slice(&args_bytes).expect("args must be valid json");
        assert_eq!(
            args_json["price_ids"].as_array().map(Vec::len),
            Some(price_ids.len()),
            "proxy step must carry every requested price id; got: {args_json}"
        );
    }

    #[tokio::test]
    async fn provider_reads_skip_empty_requests_and_propagate_source_errors() {
        let ctx = TestCtx {
            lazer_source: FakeLazerSource::new(Err(FakeError)),
            redstone_source: FakeRedStoneSource {
                outcome: Err(FakeError),
            },
            ..inert_ctx()
        };
        let oracle_id = "oracle.near".parse::<AccountId>().unwrap();
        let empty = <Dispatch as DispatchRead<GetLazerUpdate, TestCtx>>::dispatch(
            GetLazerUpdate {
                oracle_id: oracle_id.clone(),
                feed_ids: vec![],
            },
            ctx.clone(),
        )
        .await
        .unwrap();
        assert_eq!(empty, HashMap::new());
        let empty = <Dispatch as DispatchRead<GetRedStoneUpdate, TestCtx>>::dispatch(
            GetRedStoneUpdate {
                oracle_id: oracle_id.clone(),
                feed_ids: vec![],
            },
            ctx.clone(),
        )
        .await
        .unwrap();
        assert_eq!(empty, vec![]);
        let error = <Dispatch as DispatchRead<GetLazerUpdate, TestCtx>>::dispatch(
            GetLazerUpdate {
                oracle_id: oracle_id.clone(),
                feed_ids: vec![1],
            },
            ctx.clone(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, GatewayError::ExternalService(msg) if msg == "fake source unavailable")
        );
        let error = <Dispatch as DispatchRead<GetRedStoneUpdate, TestCtx>>::dispatch(
            GetRedStoneUpdate {
                oracle_id,
                feed_ids: vec!["ETH".into()],
            },
            ctx,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, GatewayError::ExternalService(msg) if msg == "fake source unavailable")
        );
    }

    const SNAPSHOT_HASH: &str = "6F3YyM29ajJxENmkyyAYWBQaTtKEJgXjwYVttj74sSSL";
    const ZERO_HASH: &str = "11111111111111111111111111111111";

    fn block_response(head: bool) -> serde_json::Value {
        let (hash, height, timestamp) = if head {
            (ZERO_HASH, 200, 200_000_000_000_u64)
        } else {
            (SNAPSHOT_HASH, 100, 100_000_000_000_u64)
        };
        serde_json::json!({
            "author": "test.near", "chunks": [], "header": {
                "approvals": [], "challenges_result": [], "chunk_mask": [], "validator_proposals": [],
                "block_merkle_root": ZERO_HASH, "challenges_root": ZERO_HASH,
                "chunk_headers_root": ZERO_HASH, "chunk_receipts_root": ZERO_HASH,
                "chunk_tx_root": ZERO_HASH, "epoch_id": ZERO_HASH, "last_ds_final_block": ZERO_HASH,
                "last_final_block": ZERO_HASH, "next_bp_hash": ZERO_HASH, "next_epoch_id": ZERO_HASH,
                "outcome_root": ZERO_HASH, "prev_hash": ZERO_HASH, "prev_state_root": ZERO_HASH,
                "random_value": ZERO_HASH, "chunks_included": 0, "gas_price": "0", "total_supply": "0",
                "hash": hash, "height": height, "latest_protocol_version": 0,
                "timestamp": timestamp, "timestamp_nanosec": timestamp.to_string(),
                "signature": "ed25519:3FPX3BmPTkfBELwhdxP6dx5kxo1AaxsbVB2yM7RPHsgbBMoZtc8MeiFiFoKp419pcRSjpknkt3Hi2nHwFkfX3XYa"
            }
        })
    }

    fn parsed_feed(id: u32, time_s: u64, price: i64) -> lazer::ParsedFeedView {
        serde_json::from_value(serde_json::json!({
            "feed_id": id, "price": price.to_string(), "exponent": -8,
            "confidence": "50", "ema_price": "123000", "ema_confidence": "40",
            "feed_update_timestamp_ns": (time_s * 1_000_000_000).to_string()
        }))
        .unwrap()
    }

    async fn snapshot_ctx(
        feeds: Vec<lazer::ParsedFeedView>,
        failure: Option<&'static str>,
    ) -> (TestCtx, wiremock::MockServer) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let heads = AtomicUsize::new(0);
        Mock::given(method("POST"))
            .respond_with(move |request: &wiremock::Request| {
                let rpc: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let method = rpc["method"].as_str().unwrap();
                let pinned = rpc["params"]["block_id"] == SNAPSHOT_HASH;
                let response = if failure == Some(method) {
                    serde_json::json!({"error": {
                        "code": -32000, "message": "fixture view failure",
                        "data": "UNKNOWN_ACCOUNT"
                    }})
                } else if method == "block" {
                    let head = !pinned && heads.fetch_add(1, Ordering::SeqCst) > 0;
                    serde_json::json!({"result": block_response(head)})
                } else {
                    let value = match rpc["params"]["method_name"].as_str().unwrap() {
                        "get_config" => serde_json::json!({
                            "max_timestamp_ahead_s": if pinned { 2 } else { 200 }
                        }),
                        "verify_update" => {
                            let mut selected = feeds.clone();
                            if !pinned {
                                for feed in &mut selected {
                                    if feed.feed_id == 2 {
                                        feed.price = Some(near_sdk::json_types::I64(999_999));
                                    }
                                }
                            }
                            serde_json::to_value(lazer::VerifiedUpdateView {
                                signer: [7; 32],
                                channel_id: 1,
                                timestamp_ns: Nanoseconds::from_secs(100),
                                feeds: selected,
                            })
                            .unwrap()
                        }
                        "get_prices" => serde_json::json!({
                            "timestamp": if pinned { "99000000000" } else { "199000000000" },
                            "prices": {
                                "ETH": if pinned { "195692129540" } else { "1" },
                                "BTC": if pinned { "6698556748915" } else { "2" }
                            }
                        }),
                        name => panic!("unexpected view {name}"),
                    };
                    serde_json::json!({"result": {
                        "block_hash": if pinned { SNAPSHOT_HASH } else { ZERO_HASH },
                        "block_height": if pinned { 100 } else { 200 },
                        "logs": [], "result": serde_json::to_vec(&value).unwrap()
                    }})
                };
                let mut response = response;
                response["jsonrpc"] = serde_json::json!("2.0");
                response["id"] = rpc["id"].clone();
                ResponseTemplate::new(200).set_body_json(response)
            })
            .mount(&server)
            .await;
        let mut network = NetworkConfig::from_rpc_url("test", server.uri().parse().unwrap());
        network.rpc_endpoints[0] =
            near_api::RPCEndpoint::new(server.uri().parse().unwrap()).with_retries(1);
        (
            TestCtx {
                near_client: NearClient::new(network),
                ..inert_ctx()
            },
            server,
        )
    }

    #[tokio::test]
    async fn lazer_preview_uses_one_snapshot_and_filters_intrinsic_invalid_feeds() {
        let mut spot_only = parsed_feed(3, 100, 55);
        spot_only.ema_price = None;
        let (ctx, _server) = snapshot_ctx(
            vec![
                parsed_feed(1, 103, 11),
                parsed_feed(2, 102, 123_456),
                spot_only,
                parsed_feed(9, 100, 99),
            ],
            None,
        )
        .await;
        let result = <Dispatch as DispatchRead<GetLazerUpdate, TestCtx>>::dispatch(
            GetLazerUpdate {
                oracle_id: "oracle.near".parse().unwrap(),
                feed_ids: vec![1, 2, 2, 3, 4],
            },
            ctx,
        )
        .await
        .unwrap();
        assert_eq!(
            result.keys().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([1, 2, 3, 4])
        );
        assert_eq!(result[&1], None);
        assert_eq!(result[&3], None);
        assert_eq!(result[&4], None);
        assert_eq!(
            result[&2],
            Some(lazer::FeedData {
                price: near_sdk::json_types::I64(123_456),
                conf: near_sdk::json_types::U64(50),
                ema: lazer::EmaData {
                    price: near_sdk::json_types::I64(123_000),
                    conf: near_sdk::json_types::U64(40)
                },
                expo: -8,
                publish_time_ns: Nanoseconds::from_secs(102),
            })
        );
    }

    #[rstest]
    #[case::newer_then_older(102, 101, true, 11, 102)]
    #[case::older_then_newer(101, 102, true, 22, 102)]
    #[case::valid_then_invalid(101, 102, false, 11, 101)]
    #[case::equal_first_wins(101, 101, true, 11, 101)]
    #[tokio::test]
    async fn lazer_preview_selects_first_valid_then_strictly_newer(
        #[case] first_time: u64,
        #[case] second_time: u64,
        #[case] second_valid: bool,
        #[case] expected_price: i64,
        #[case] expected_time: u64,
    ) {
        let mut second = parsed_feed(1, second_time, 22);
        if !second_valid {
            second.confidence = Some(near_sdk::json_types::I64(0));
        }
        let (ctx, _server) = snapshot_ctx(vec![parsed_feed(1, first_time, 11), second], None).await;
        let result = <Dispatch as DispatchRead<GetLazerUpdate, TestCtx>>::dispatch(
            GetLazerUpdate {
                oracle_id: "oracle.near".parse().unwrap(),
                feed_ids: vec![1],
            },
            ctx,
        )
        .await
        .unwrap();
        let selected = result[&1].as_ref().unwrap();
        assert_eq!(selected.price.0, expected_price);
        assert_eq!(
            selected.publish_time_ns,
            Nanoseconds::from_secs(expected_time)
        );
    }

    #[tokio::test]
    async fn redstone_preview_preserves_order_duplicates_and_snapshot_clock() {
        let (ctx, _server) = snapshot_ctx(vec![], None).await;
        let ids: Vec<redstone::FeedId> = ["BTC", "ETH", "BTC"].map(Into::into).to_vec();
        let result = <Dispatch as DispatchRead<GetRedStoneUpdate, TestCtx>>::dispatch(
            GetRedStoneUpdate {
                oracle_id: "oracle.near".parse().unwrap(),
                feed_ids: ids.clone(),
            },
            ctx,
        )
        .await
        .unwrap();
        let expected = ids
            .into_iter()
            .zip([6_698_556_748_915_u64, 195_692_129_540, 6_698_556_748_915])
            .map(
                |(feed_id, price)| templar_gateway_methods_spec::redstone::PriceDataEntry {
                    feed_id,
                    data: redstone::FeedData {
                        price: templar_common::primitive_types::U256::from(price).into(),
                        package_timestamp: Nanoseconds::from_secs(99),
                        write_timestamp: Nanoseconds::from_secs(100),
                    },
                },
            )
            .collect::<Vec<_>>();
        assert_eq!(result, expected);
    }

    #[rstest]
    #[case("block")]
    #[case("query")]
    #[tokio::test]
    async fn provider_reads_propagate_chain_errors(#[case] failure: &'static str) {
        let (ctx, _server) = snapshot_ctx(vec![], Some(failure)).await;
        let oracle_id: AccountId = "oracle.near".parse().unwrap();
        let lazer = <Dispatch as DispatchRead<GetLazerUpdate, TestCtx>>::dispatch(
            GetLazerUpdate {
                oracle_id: oracle_id.clone(),
                feed_ids: vec![1],
            },
            ctx.clone(),
        )
        .await
        .unwrap_err();
        let redstone = <Dispatch as DispatchRead<GetRedStoneUpdate, TestCtx>>::dispatch(
            GetRedStoneUpdate {
                oracle_id: oracle_id.clone(),
                feed_ids: vec!["ETH".into()],
            },
            ctx,
        )
        .await
        .unwrap_err();
        for error in [lazer, redstone] {
            if failure == "query" {
                assert!(matches!(error, GatewayError::AccountNotFound(id) if id == oracle_id));
            } else {
                assert!(
                    matches!(error, GatewayError::NearQuery(message) if message.contains("fixture view failure"))
                );
            }
        }
    }

    #[tokio::test]
    async fn redstone_preview_rejects_incomplete_verified_payload() {
        let (ctx, _server) = snapshot_ctx(vec![], None).await;
        let error = <Dispatch as DispatchRead<GetRedStoneUpdate, TestCtx>>::dispatch(
            GetRedStoneUpdate {
                oracle_id: "oracle.near".parse().unwrap(),
                feed_ids: vec!["ETH".into(), "MISSING".into()],
            },
            ctx,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, GatewayError::ExternalService(message)
            if message == "verified RedStone payload is missing requested feed MISSING"));
    }
}
