use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use near_account_id::AccountId;
use templar_common::oracle::{pyth::PriceIdentifier, redstone};
use templar_gateway_core::{
    client::{lst_oracle::GetTransformerArgs, proxy_oracle::GetProxyArgs},
    plan_pyth_pro_update, plan_pyth_update, plan_redstone_write_prices, query_contract_kind,
    GatewayError, GatewayResult, HasNearClient, OperationPlan, OraclePayloadSource, PlanWrite,
};
use templar_gateway_methods_spec::oracle::OracleContractKind;
use templar_gateway_oracle_updates_spec::oracle::{
    UpdateLazer, UpdatePrices, UpdatePyth, UpdateRedStone,
};
use templar_gateway_types::{ContractKind, ManagedAccountId};
use templar_proxy_oracle_kernel::proxy;
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_common::request::{LazerRequest, OracleRequest};

use crate::{Dispatch, ProvidesLazerSource, ProvidesPythSource, ProvidesRedStoneSource};

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
        plan_pyth_update(
            ctx.near_client(),
            request.signer_account_id,
            body.oracle_id,
            body.vaa.0,
        )
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
        let feed_id = body.feed_id;
        tracing::debug!(
            oracle_id = %body.oracle_id,
            feed_id = %feed_id,
            "fetching RedStone payload for gateway oracle update"
        );
        let payload = OraclePayloadSource::fetch_payload(ctx.redstone_source(), &[feed_id.clone()])
            .await
            .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
        plan_redstone_write_prices(
            ctx.near_client(),
            request.signer_account_id,
            body.oracle_id,
            vec![feed_id],
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
        let payload = OraclePayloadSource::fetch_payload(ctx.lazer_source(), &[body.feed_id])
            .await
            .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
        plan_pyth_pro_update(
            ctx.near_client(),
            request.signer_account_id,
            body.oracle_id,
            payload,
        )
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
        let requests =
            resolve_update_requests(&ctx, request.body.oracle_id, request.body.price_ids).await?;

        plan_grouped_updates(&ctx, request.signer_account_id, requests).await
    }
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
        tracing::debug!(
            %oracle_id,
            price_count = price_ids.len(),
            "fetching Pyth payload for gateway oracle update"
        );
        let vaa = OraclePayloadSource::fetch_payload(ctx.pyth_source(), &price_ids)
            .await
            .map_err(|error| GatewayError::HttpRequest(error.to_string()))?;
        steps.push(plan_pyth_update(
            ctx.near_client(),
            signer_account_id.clone(),
            oracle_id,
            vaa,
        )?);
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
        tracing::debug!(
            %oracle_id,
            feed_count = feed_ids.len(),
            "fetching Pyth Pro/Lazer payload for gateway oracle update"
        );
        let payload = OraclePayloadSource::fetch_payload(ctx.lazer_source(), &feed_ids)
            .await
            .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
        steps.push(plan_pyth_pro_update(
            ctx.near_client(),
            signer_account_id.clone(),
            oracle_id,
            payload,
        )?);
    }

    Ok(OperationPlan { steps })
}

async fn resolve_update_requests<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
    price_ids: Vec<PriceIdentifier>,
) -> GatewayResult<Vec<OracleRequest>> {
    let kind = query_oracle_kind(ctx, oracle_id.clone()).await?;
    let mut requests = BTreeSet::new();

    for price_id in price_ids {
        requests.extend(resolve_dependencies(ctx, oracle_id.clone(), price_id, &kind).await?);
    }

    Ok(requests.into_iter().collect())
}

async fn get_proxy<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
    id: PriceIdentifier,
) -> GatewayResult<Option<proxy::Proxy<Source>>> {
    ctx.near_client()
        .proxy_oracle(oracle_id)
        .cached_get_proxy(GetProxyArgs { id })
        .await
}

async fn query_oracle_kind<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
) -> GatewayResult<OracleContractKind> {
    match query_contract_kind(ctx, oracle_id.clone()).await? {
        ContractKind::PythOracle | ContractKind::RedstoneOracle => Ok(OracleContractKind::Direct),
        ContractKind::ProxyOracle => Ok(OracleContractKind::Proxy),
        ContractKind::LstOracle => {
            let pyth_id = ctx
                .near_client()
                .lst_oracle(oracle_id)
                .cached_oracle_id()
                .await?;
            Ok(OracleContractKind::Lst { pyth_id })
        }
        other => Err(GatewayError::NearQuery(format!(
            "contract kind {other:?} is not an oracle contract"
        ))),
    }
}

async fn resolve_dependencies<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
    price_id: PriceIdentifier,
    kind: &OracleContractKind,
) -> GatewayResult<Vec<OracleRequest>> {
    match kind.clone() {
        OracleContractKind::Direct => Ok(vec![OracleRequest::pyth(oracle_id, price_id)]),
        OracleContractKind::Lst { pyth_id } => {
            let transformer = ctx
                .near_client()
                .lst_oracle(oracle_id)
                .cached_get_transformer(GetTransformerArgs {
                    price_identifier: price_id,
                })
                .await?;
            Ok(vec![transformer.map_or_else(
                || OracleRequest::pyth(pyth_id.clone(), price_id),
                |transformer| OracleRequest::pyth(pyth_id.clone(), transformer.price_id),
            )])
        }
        OracleContractKind::Proxy => {
            let proxy = get_proxy(ctx, oracle_id, price_id).await?.ok_or_else(|| {
                GatewayError::NearQuery("price identifier not found on proxy oracle".to_owned())
            })?;
            let requests = proxy
                .sources()
                .map(|source| match source {
                    Source::Request(request) => request.clone(),
                    Source::Transformer(transformer) => transformer.request.clone(),
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            if requests.is_empty() {
                return Err(GatewayError::NearQuery(
                    "proxy oracle returned empty proxy definition".to_owned(),
                ));
            }
            Ok(requests)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use near_api::types::transaction::actions::Action;
    use near_api::NetworkConfig;
    use templar_gateway_core::{NearClient, PlannedTransaction};
    use templar_gateway_types::common::WriteRequest;
    use thiserror::Error;

    #[derive(Debug, Clone, Error)]
    enum FakeError {
        #[error("fake cache miss")]
        CacheMiss,
        #[error("fake stale payload")]
        StalePayload,
    }

    #[derive(Clone)]
    struct FakeLazerSource {
        outcome: Result<Vec<u8>, FakeError>,
    }

    #[async_trait]
    impl OraclePayloadSource for FakeLazerSource {
        type PriceId = u32;
        type Error = FakeError;

        async fn fetch_payload(&self, _price_ids: &[u32]) -> Result<Vec<u8>, FakeError> {
            self.outcome.clone()
        }
    }

    #[derive(Clone)]
    struct FakePythSource {
        payload: Vec<u8>,
    }

    #[async_trait]
    impl OraclePayloadSource for FakePythSource {
        type PriceId = PriceIdentifier;
        type Error = FakeError;

        async fn fetch_payload(
            &self,
            _price_ids: &[PriceIdentifier],
        ) -> Result<Vec<u8>, FakeError> {
            Ok(self.payload.clone())
        }
    }

    #[derive(Clone)]
    struct FakeRedStoneSource {
        payload: Vec<u8>,
    }

    #[async_trait]
    impl OraclePayloadSource for FakeRedStoneSource {
        type PriceId = redstone::FeedId;
        type Error = FakeError;

        async fn fetch_payload(
            &self,
            _feed_ids: &[redstone::FeedId],
        ) -> Result<Vec<u8>, FakeError> {
            Ok(self.payload.clone())
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

    #[tokio::test]
    async fn update_lazer_plans_one_adapter_write() {
        let payload = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let ctx = TestCtx {
            near_client: test_client(),
            pyth_source: FakePythSource {
                payload: Vec::new(),
            },
            redstone_source: FakeRedStoneSource {
                payload: Vec::new(),
            },
            lazer_source: FakeLazerSource {
                outcome: Ok(payload.clone()),
            },
        };
        let oracle_id: AccountId = "pyth-pro.near".parse().expect("valid account id");
        let request = WriteRequest {
            signer_account_id: signer_id(),
            idempotency_key: None,
            body: UpdateLazer {
                oracle_id: oracle_id.clone(),
                feed_id: 7,
            },
        };

        let plan = <Dispatch as PlanWrite<UpdateLazer, TestCtx>>::plan(request, ctx)
            .await
            .expect("UpdateLazer plan must succeed with a fresh payload");

        assert_eq!(
            plan.steps.len(),
            1,
            "UpdateLazer must plan exactly one step"
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
    async fn update_lazer_propagates_cache_miss() {
        let ctx = TestCtx {
            near_client: test_client(),
            pyth_source: FakePythSource {
                payload: Vec::new(),
            },
            redstone_source: FakeRedStoneSource {
                payload: Vec::new(),
            },
            lazer_source: FakeLazerSource {
                outcome: Err(FakeError::CacheMiss),
            },
        };
        let request = WriteRequest {
            signer_account_id: signer_id(),
            idempotency_key: None,
            body: UpdateLazer {
                oracle_id: "pyth-pro.near".parse().unwrap(),
                feed_id: 7,
            },
        };

        let error = <Dispatch as PlanWrite<UpdateLazer, TestCtx>>::plan(request, ctx)
            .await
            .expect_err("a Lazer cache miss must surface as a plan error");

        assert!(
            matches!(error, GatewayError::ExternalService(ref msg) if msg.contains("cache miss")),
            "expected ExternalService carrying the cache-miss detail, got {error:?}"
        );
    }

    #[tokio::test]
    async fn update_prices_groups_lazer_separately() {
        let pyth_payload = vec![0x11_u8; 16];
        let lazer_payload = vec![0x22_u8; 24];
        let ctx = TestCtx {
            near_client: test_client(),
            pyth_source: FakePythSource {
                payload: pyth_payload.clone(),
            },
            redstone_source: FakeRedStoneSource {
                payload: Vec::new(),
            },
            lazer_source: FakeLazerSource {
                outcome: Ok(lazer_payload.clone()),
            },
        };
        let pyth_oracle: AccountId = "pyth.near".parse().unwrap();
        let lazer_oracle: AccountId = "pyth-pro.near".parse().unwrap();
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
    async fn update_prices_lazer_stale_is_hard_error() {
        let ctx = TestCtx {
            near_client: test_client(),
            pyth_source: FakePythSource {
                payload: vec![0x11_u8; 16],
            },
            redstone_source: FakeRedStoneSource {
                payload: Vec::new(),
            },
            lazer_source: FakeLazerSource {
                outcome: Err(FakeError::StalePayload),
            },
        };
        let lazer_oracle: AccountId = "pyth-pro.near".parse().unwrap();
        let requests = vec![OracleRequest::lazer(lazer_oracle, 7)];

        let error = plan_grouped_updates(&ctx, signer_id(), requests)
            .await
            .expect_err("a stale Lazer payload must be a hard plan error");

        assert!(
            matches!(error, GatewayError::ExternalService(ref msg) if msg.contains("stale")),
            "expected ExternalService carrying the stale detail, got {error:?}"
        );
    }
}
