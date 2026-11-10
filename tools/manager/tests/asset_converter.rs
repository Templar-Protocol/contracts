//! Real token resolvers, receipt graphs and balances are settlement evidence;
//! the converter's requested-refund event is deliberately not sufficient.

use std::collections::BTreeMap;

use anyhow::{ensure, Context as _, Result};
use near_account_id::AccountId;
use near_api::Contract;
use serde_json::{json, Value};
use templar_common::asset::{BorrowAsset, FungibleAsset};
use templar_gateway_methods_spec::{contract, token, tx};
use templar_gateway_testing::{failed_receipts, SandboxHarness, TEST_FINALITY_POLICY};
use templar_gateway_types::{
    common::{ContractArgs, TxExecutionStatus, WriteOperationResult},
    operation::ReceiptStatus,
    ContractMethodName, ManagedAccountId, NearGas, NearToken, OperationStatus,
};

type Asset = FungibleAsset<BorrowAsset>;
const INVENTORY: u128 = 100_000;
const INPUT: u128 = 101; // Odd input also proves partial-refund rounding.

struct Converter {
    id: AccountId,
    owner: ManagedAccountId,
    sender: ManagedAccountId,
    other: ManagedAccountId,
    // Alternating old/new assets for routes 0 and 1, then the fault output.
    assets: Vec<Asset>,
}

impl Converter {
    async fn new(
        harness: &SandboxHarness,
        input_mt: bool,
        output_mt: bool,
        market: Option<(ManagedAccountId, Asset)>,
    ) -> Result<Self> {
        let owner = harness.create_user("converter-owner").await?;
        let other = harness.create_user("converter-other").await?;
        let (sender, primary_output) = match market {
            Some((account, asset)) => (account, Some(asset)),
            None => (harness.create_user("converter-sender").await?, None),
        };
        let old = deploy_assets(harness, "converter-old", input_mt).await?;
        let new = deploy_assets(harness, "converter-new", output_mt).await?;
        let assets = vec![
            old[0].clone(),
            primary_output.unwrap_or_else(|| new[0].clone()),
            old[1].clone(),
            new[1].clone(),
            new[2].clone(),
        ];
        let routes = json!([
            {"id":0,"old_asset":assets[0],"new_asset":assets[1]},
            {"id":1,"old_asset":assets[2],"new_asset":assets[3]}
        ]);
        let id = deploy_converter(harness, &owner, routes).await?;
        let converter = ManagedAccountId(id.clone());
        for (index, asset) in assets.iter().enumerate() {
            for account in [&sender, &converter, &other] {
                register(harness, account, asset).await?;
            }
            let holder = if index == 0 || index == 2 {
                &sender
            } else {
                &converter
            };
            fund(harness, holder, asset, INVENTORY).await?;
        }
        Ok(Self {
            id,
            owner,
            sender,
            other,
            assets,
        })
    }

    async fn mode(&self, harness: &SandboxHarness, mode: Value) -> Result<()> {
        let result = harness
            .call_function(
                &self.owner,
                &self.id,
                "set_failure_mode",
                json!({"mode":mode}),
            )
            .await?;
        ensure!(
            failed_receipts(&result).next().is_none(),
            "owner mode change failed"
        );
        Ok(())
    }

    async fn convert(
        &self,
        harness: &SandboxHarness,
        route: usize,
    ) -> Result<WriteOperationResult> {
        self.convert_message(harness, route, INPUT, json!({"route":route}).to_string())
            .await
    }

    async fn convert_message(
        &self,
        harness: &SandboxHarness,
        route: usize,
        amount: u128,
        msg: String,
    ) -> Result<WriteOperationResult> {
        harness
            .try_execute(
                &self.sender,
                token::TransferCall {
                    token: (&self.assets[route * 2]).into(),
                    receiver_id: self.id.clone(),
                    amount: amount.into(),
                    msg,
                    memo: None,
                },
            )
            .await
    }

    async fn balances(&self, harness: &SandboxHarness) -> Result<Vec<[u128; 3]>> {
        let mut balances = Vec::with_capacity(self.assets.len());
        for asset in &self.assets {
            let mut row = [0; 3];
            for (index, account) in [&self.sender.0, &self.id, &self.other.0]
                .into_iter()
                .enumerate()
            {
                row[index] = harness
                    .client()?
                    .read(token::GetBalanceOf {
                        token: asset.into(),
                        account_id: account.clone(),
                    })
                    .await?
                    .0;
            }
            balances.push(row);
        }
        Ok(balances)
    }

    async fn settlement(&self, harness: &SandboxHarness) -> Result<Value> {
        view(harness, &self.id, "get_last_settlement").await
    }
}

async fn deploy_assets(harness: &SandboxHarness, label: &str, mt: bool) -> Result<Vec<Asset>> {
    if mt {
        let token_ids = [
            "nep141:remote.chain:opaque:0xAbCd",
            "nep245:remote.chain:nested:second:asset",
            "wrong:output:token:on:same:contract",
        ];
        let (id, signer) = harness
            .create_account_via_tx(label, NearToken::from_near(100))
            .await?;
        Contract::deploy(id.clone())
            .use_code(templar_gateway_testing::wasm::mt().await.to_vec())
            .with_init_call("new", json!({"extra_token_ids":token_ids}))?
            .with_signer(signer)
            .wait_until(TEST_FINALITY_POLICY.transaction_status())
            .send_to(&harness.network)
            .await?
            .assert_success();
        Ok(token_ids
            .into_iter()
            .map(|token_id| Asset::nep245(id.clone(), token_id.to_owned()))
            .collect())
    } else {
        let mut assets = Vec::new();
        for index in 0..3 {
            let id = harness
                .deploy_ft(&format!("{label}-{index}"), "Converter fixture", "FIX")
                .await?;
            assets.push(Asset::nep141(id));
        }
        Ok(assets)
    }
}

async fn deploy_converter(
    harness: &SandboxHarness,
    owner: &ManagedAccountId,
    routes: Value,
) -> Result<AccountId> {
    let (id, signer) = harness
        .create_account_via_tx("converter", NearToken::from_near(100))
        .await?;
    Contract::deploy(id.clone())
        .use_code(
            templar_gateway_testing::wasm::mock_asset_converter()
                .await
                .to_vec(),
        )
        .with_init_call("new", json!({"owner_id":owner.0,"routes":routes}))?
        .with_signer(signer)
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(&harness.network)
        .await?
        .assert_success();
    Ok(id)
}

async fn register(
    harness: &SandboxHarness,
    account: &ManagedAccountId,
    asset: &Asset,
) -> Result<()> {
    if asset.nep245_token_id().is_none() {
        harness
            .storage_deposit(account, asset.contract_id(), NearToken::from_millinear(10))
            .await?;
    }
    Ok(())
}

async fn fund(
    harness: &SandboxHarness,
    account: &ManagedAccountId,
    asset: &Asset,
    amount: u128,
) -> Result<()> {
    if let Some(token_id) = asset.nep245_token_id() {
        harness
            .mint_mt(account, asset.contract_id(), token_id, amount)
            .await?;
    } else {
        harness.mint(account, asset.contract_id(), amount).await?;
    }
    Ok(())
}

async fn view(harness: &SandboxHarness, id: &AccountId, method: &str) -> Result<Value> {
    Ok(harness
        .client()?
        .read(contract::ViewFunction {
            contract_id: id.clone(),
            method_name: ContractMethodName(method.to_owned()),
            args: ContractArgs::Json(json!({})),
        })
        .await?)
}

async fn finalized_transaction(
    harness: &SandboxHarness,
    sender: &AccountId,
    result: &WriteOperationResult,
) -> Result<tx::GetResult> {
    let hash = result
        .operation
        .steps
        .first()
        .and_then(|step| step.tx_hash())
        .context("missing submitted transaction")?;
    Ok(harness
        .client()?
        .read(tx::Get {
            tx_hash: hash,
            sender_account_id: sender.clone(),
            wait_until: Some(TxExecutionStatus::Final),
            encoding: tx::ValueEncoding::Json,
        })
        .await?)
}

/// Fetch finality independently and assert the OLD token's actual used amount.
/// A successful token result is compatible with a failed converter receipt.
async fn resolution(
    harness: &SandboxHarness,
    fixture: &Converter,
    route: usize,
    result: &WriteOperationResult,
    used: u128,
    converter_failed: bool,
) -> Result<tx::GetResult> {
    ensure!(
        result.operation.status == OperationStatus::Succeeded,
        "token resolver did not complete: {result:?}"
    );
    let evidence = finalized_transaction(harness, &fixture.sender.0, result).await?;
    ensure!(
        evidence.status == tx::Status::Succeeded,
        "nonterminal resolver: {evidence:?}"
    );
    let asset = &fixture.assets[route * 2];
    let expected = if asset.nep245_token_id().is_some() {
        json!([used.to_string()])
    } else {
        json!(used.to_string())
    };
    ensure!(
        evidence.return_value == Some(tx::ReturnValue::Json(expected.clone())),
        "native token used amount mismatch: {evidence:?}"
    );
    ensure!(
        evidence
            .receipts
            .iter()
            .any(|receipt| receipt.executor_id == *asset.contract_id()
                && receipt.status == ReceiptStatus::Succeeded
                && receipt.return_value == Some(tx::ReturnValue::Json(expected.clone()))),
        "missing authoritative token result receipt"
    );
    if converter_failed {
        ensure!(
            evidence.failed_receipts == vec![fixture.id.clone()],
            "unexpected failure graph: {evidence:?}"
        );
    } else {
        ensure!(
            evidence.failed_receipts.is_empty(),
            "unexpected failed child receipt: {evidence:?}"
        );
    }
    Ok(evidence)
}

#[rstest::rstest]
#[case::ft_to_ft(false, false)]
#[case::ft_to_mt(false, true)]
#[case::mt_to_ft(true, false)]
#[case::mt_to_mt(true, true)]
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one ordered token-standard matrix checks each fault against its exact resolver and balance evidence"
)]
async fn requires_sandbox_converter_conserves_raw_units_and_exposes_fault_settlement(
    #[case] input_mt: bool,
    #[case] output_mt: bool,
) -> Result<()> {
    let harness = SandboxHarness::start_owned().await?;
    let fixture = Converter::new(&harness, input_mt, output_mt, None).await?;
    let profile = view(&harness, &fixture.id, "get_profile").await?;
    ensure!(
        profile
            == json!({
                "schema":1,"profile":"local-one-to-one","settlement_rule":"one-to-one-raw","requires_registration":false,
                "routes":[
                    {"id":0,"old_asset":fixture.assets[0],"new_asset":fixture.assets[1]},
                    {"id":1,"old_asset":fixture.assets[2],"new_asset":fixture.assets[3]}
                ]
            }),
        "profile must preserve exact asset identities and opaque token IDs"
    );
    ensure!(
        fixture.settlement(&harness).await?.is_null(),
        "fixture fabricated a settlement"
    );
    let modes = [
        (json!("Normal"), INPUT, Some(1), 0, INPUT, false),
        (json!("Panic"), 0, None, 0, 0, true),
        (json!("RefundAll"), 0, None, 0, 0, false),
        (
            json!("RefundPartial"),
            INPUT / 2,
            Some(1),
            0,
            INPUT / 2,
            false,
        ),
        (json!("MissingOutput"), INPUT, None, 0, 0, false),
        (
            json!({"WrongRecipient":fixture.other.0}),
            INPUT,
            Some(1),
            2,
            INPUT,
            false,
        ),
        (
            json!({"WrongOutputToken":fixture.assets[4]}),
            INPUT,
            Some(4),
            0,
            INPUT,
            false,
        ),
    ];
    let mut sequence = 0_u64;
    for (mode, consumed, output_asset, recipient, output, failed) in modes {
        fixture.mode(&harness, mode.clone()).await?;
        let before = fixture.balances(&harness).await?;
        let prior = fixture.settlement(&harness).await?;
        let result = fixture.convert(&harness, 0).await?;
        let evidence = resolution(&harness, &fixture, 0, &result, consumed, failed).await?;
        let mut expected = before;
        expected[0][0] -= consumed;
        expected[0][1] += consumed;
        if let Some(index) = output_asset {
            expected[index][1] -= output;
            expected[index][recipient] += output;
        }
        ensure!(
            fixture.balances(&harness).await? == expected,
            "actual output/refund balances mismatch for {mode}"
        );
        if failed {
            ensure!(
                fixture.settlement(&harness).await? == prior,
                "panicked conversion changed settlement state"
            );
            continue;
        }
        sequence += 1;
        let settlement = fixture.settlement(&harness).await?;
        let new_asset = &fixture.assets[output_asset.unwrap_or(1)];
        let recipient_id = if recipient == 2 {
            &fixture.other.0
        } else {
            &fixture.sender.0
        };
        let expected_settlement = json!({
            "sequence":sequence,"route":0,"sender":fixture.sender.0,"recipient":recipient_id,
            "old_asset":fixture.assets[0],"new_asset":new_asset,"mode":mode,"input":INPUT.to_string(),
            "consumed":consumed.to_string(),"refunded":(INPUT-consumed).to_string(),"output":output.to_string(),
            "refund_is_requested":true,"output_succeeded":output > 0
        });
        ensure!(
            settlement == expected_settlement,
            "settlement disagrees with real token evidence: {settlement}"
        );
        ensure!(
            evidence
                .receipts
                .iter()
                .filter(|receipt| receipt.executor_id == fixture.id)
                .flat_map(|receipt| &receipt.logs)
                .filter_map(|log| log.strip_prefix("EVENT_JSON:"))
                .filter_map(|event| serde_json::from_str::<Value>(event).ok())
                .any(|event| event["standard"] == "templar_test_asset_converter"
                    && event["data"] == json!([expected_settlement])),
            "settlement event is absent or disagrees with retained evidence"
        );
    }

    // Success on one inventory is not rolled back or consumed again when the
    // independently nonce-ordered second route fails and is later resumed.
    fixture.mode(&harness, json!("Normal")).await?;
    let result = fixture.convert(&harness, 0).await?;
    resolution(&harness, &fixture, 0, &result, INPUT, false).await?;
    let first_complete = fixture.balances(&harness).await?;
    let first_settlement = fixture.settlement(&harness).await?;
    fixture.mode(&harness, json!("Panic")).await?;
    let result = fixture.convert(&harness, 1).await?;
    resolution(&harness, &fixture, 1, &result, 0, true).await?;
    ensure!(
        fixture.balances(&harness).await? == first_complete,
        "second-route panic affected completed first route"
    );
    ensure!(
        fixture.settlement(&harness).await? == first_settlement,
        "failed second route overwrote first-route proof"
    );
    fixture.mode(&harness, json!("Normal")).await?;
    let result = fixture.convert(&harness, 1).await?;
    resolution(&harness, &fixture, 1, &result, INPUT, false).await?;
    let mut expected = first_complete;
    expected[2][0] -= INPUT;
    expected[2][1] += INPUT;
    expected[3][1] -= INPUT;
    expected[3][0] += INPUT;
    ensure!(
        fixture.balances(&harness).await? == expected,
        "resuming second inventory changed first inventory"
    );
    let settlement = fixture.settlement(&harness).await?;
    ensure!(
        settlement["route"] == 1
            && settlement["old_asset"] == serde_json::to_value(&fixture.assets[2])?
            && settlement["new_asset"] == serde_json::to_value(&fixture.assets[3])?,
        "second route identity was not preserved"
    );
    Ok(())
}

#[rstest::rstest]
#[case::ft(false)]
#[case::mt(true)]
#[tokio::test]
async fn requires_sandbox_converter_rejects_untrusted_inputs_without_consuming_inventory(
    #[case] mt: bool,
) -> Result<()> {
    let harness = SandboxHarness::start_owned().await?;
    let fixture = Converter::new(&harness, mt, mt, None).await?;
    let before = fixture.balances(&harness).await?;
    let result = harness
        .try_call_function(
            &fixture.other,
            &fixture.id,
            "set_failure_mode",
            json!({"mode":{"WrongOutputToken":fixture.assets[4]}}),
        )
        .await?;
    ensure!(
        result.operation.status == OperationStatus::Failed
            && failed_receipts(&result).any(|receipt| receipt.contract_id == fixture.id),
        "non-owner configured a fault mode"
    );
    // Every invalid request travels through the actual token receive hook and
    // resolver; wrong route pairs include same-contract, different opaque IDs.
    for msg in [
        json!({"route":1}).to_string(),
        json!({"route":999}).to_string(),
        json!({"route":0,"unexpected":true}).to_string(),
        "not-json".to_owned(),
    ] {
        let result = fixture.convert_message(&harness, 0, INPUT, msg).await?;
        resolution(&harness, &fixture, 0, &result, 0, true).await?;
        ensure!(
            fixture.balances(&harness).await? == before,
            "invalid pair/message did not refund actual input"
        );
        ensure!(
            fixture.settlement(&harness).await?.is_null(),
            "invalid pair/message recorded a conversion"
        );
    }
    if mt {
        // MT invokes the converter and its resolver refunds the rejected zero input.
        let zero = fixture
            .convert_message(&harness, 0, 0, json!({"route":0}).to_string())
            .await?;
        resolution(&harness, &fixture, 0, &zero, 0, true).await?;
        ensure!(
            fixture.balances(&harness).await? == before
                && fixture.settlement(&harness).await?.is_null(),
            "zero input mutated conversion state"
        );
    }
    // A normal request still succeeds: rejected fault configuration was not
    // silently persisted, and allowed predecessor/pair remains usable.
    let result = fixture
        .convert_message(&harness, 0, 1, json!({"route":0}).to_string())
        .await?;
    resolution(&harness, &fixture, 0, &result, 1, false).await?;
    let mut expected = before;
    expected[0][0] -= 1;
    expected[0][1] += 1;
    expected[1][1] -= 1;
    expected[1][0] += 1;
    ensure!(
        fixture.balances(&harness).await? == expected,
        "authorized route did not settle exactly"
    );

    let before = fixture.balances(&harness).await?;
    let settlement = fixture.settlement(&harness).await?;
    let token_signer = ManagedAccountId(fixture.assets[0].contract_id().to_owned());
    let (method, valid) = if mt {
        (
            "mt_on_transfer",
            json!({"sender_id":fixture.sender.0,"previous_owner_ids":[fixture.sender.0],
            "token_ids":[fixture.assets[0].nep245_token_id()],"amounts":["1"],"msg":"{\"route\":0}"}),
        )
    } else {
        (
            "ft_on_transfer",
            json!({"sender_id":fixture.sender.0,"amount":"1","msg":"{\"route\":0}"}),
        )
    };
    let result = harness
        .try_call_function(&fixture.other, &fixture.id, method, valid.clone())
        .await?;
    assert_receiver_rejected(&result, &fixture.id)?;
    let mut zero = valid.clone();
    zero[if mt { "amounts" } else { "amount" }] = if mt { json!(["0"]) } else { json!("0") };
    let result = harness
        .try_call_function(&token_signer, &fixture.id, method, zero)
        .await?;
    assert_receiver_rejected(&result, &fixture.id)?;
    if mt {
        let variants = [
            ("previous_owner_ids", json!([fixture.other.0])),
            ("previous_owner_ids", json!([])),
            (
                "previous_owner_ids",
                json!([fixture.sender.0, fixture.sender.0]),
            ),
            ("token_ids", json!([])),
            ("token_ids", json!(["mt_borrow", "mt_collateral"])),
            ("amounts", json!([])),
            ("amounts", json!(["1", "1"])),
        ];
        for (field, value) in variants {
            let mut args = valid.clone();
            args[field] = value;
            let result = harness
                .try_call_function(&token_signer, &fixture.id, method, args)
                .await?;
            assert_receiver_rejected(&result, &fixture.id)?;
        }
        let result = harness.try_execute(&fixture.sender, tx::FunctionCall {
            receiver_id: fixture.assets[0].contract_id().to_owned(),
            method_name: ContractMethodName("mt_batch_transfer_call".to_owned()),
            args: ContractArgs::Json(json!({"receiver_id":fixture.id,
                "token_ids":[fixture.assets[0].nep245_token_id(),fixture.assets[2].nep245_token_id()],
                "amounts":[INPUT.to_string(),INPUT.to_string()],"msg":"{\"route\":0}"})),
            gas: NearGas::from_tgas(300), deposit: NearToken::from_yoctonear(1),
        }).await?;
        ensure!(
            result.operation.status == OperationStatus::Succeeded,
            "multi-input resolver did not complete"
        );
        ensure!(
            failed_receipts(&result)
                .map(|receipt| &receipt.contract_id)
                .collect::<Vec<_>>()
                == vec![&fixture.id],
            "multi-input hook did not reject at converter"
        );
        let used: Value = result
            .operation
            .final_outcome()
            .context("batch token resolver missing")?
            .return_value_json()?
            .context("batch token used amounts missing")?;
        ensure!(
            used == json!(["0", "0"]),
            "native multi-input resolver did not refund both inventories"
        );
    }
    ensure!(
        fixture.balances(&harness).await? == before,
        "invalid receive hook changed inventory"
    );
    ensure!(
        fixture.settlement(&harness).await? == settlement,
        "invalid receive hook changed settlement state"
    );
    Ok(())
}

fn assert_receiver_rejected(result: &WriteOperationResult, converter: &AccountId) -> Result<()> {
    ensure!(
        result.operation.status == OperationStatus::Failed,
        "invalid direct receiver call succeeded: {result:?}"
    );
    ensure!(
        failed_receipts(result).any(|receipt| &receipt.contract_id == converter),
        "missing rejected converter receipt"
    );
    Ok(())
}

async fn storage(
    harness: &SandboxHarness,
    account: &AccountId,
) -> Result<BTreeMap<Vec<u8>, Vec<u8>>> {
    let client = templar_gateway_core::NearClient::with_finality_policy(
        harness.network.clone(),
        templar_gateway_core::FinalityPolicy::Final,
    );
    Ok(client
        .contract(account.clone())
        .state_with_prefix(Vec::new())
        .await?
        .into_iter()
        .map(|(key, value)| (key.0, value.0))
        .collect())
}

#[rstest::rstest]
#[case::ft_output(false)]
#[case::mt_output(true)]
#[tokio::test]
async fn requires_sandbox_converter_deposit_back_credits_real_market_supply_and_mutates_storage(
    #[case] output_mt: bool,
) -> Result<()> {
    let harness = SandboxHarness::start_owned().await?;
    let supply_range = (1_u128, None::<u128>).try_into()?;
    let market = harness
        .deploy_full_market_std(output_mt, false, |configuration| {
            configuration.supply_range = supply_range;
        })
        .await?;
    let sender = ManagedAccountId(market.market_id.clone());
    let fixture = Converter::new(
        &harness,
        !output_mt,
        output_mt,
        Some((sender, market.configuration.borrow_asset.clone())),
    )
    .await?;
    let converter = ManagedAccountId(fixture.id.clone());
    let registration = harness
        .storage_deposit_min(&converter, &market.market_id)
        .await?;
    // Harness writes finish optimistically, but storage() reads finalized state.
    // Finalize this last market setup write before freezing the raw baseline;
    // resolution() also finalizes each conversion before its checkpoint.
    let registration = finalized_transaction(&harness, &fixture.id, &registration).await?;
    ensure!(
        registration.status == tx::Status::Succeeded && registration.failed_receipts.is_empty(),
        "market storage registration did not finalize successfully: {registration:?}"
    );
    ensure!(
        harness
            .get_supply_position(&market, &fixture.id)
            .await?
            .is_none(),
        "converter already had market credit"
    );
    let before_storage = storage(&harness, &market.market_id).await?;
    let before_balances = fixture.balances(&harness).await?;
    // Plain transfer cannot create a deposit even though the market receives
    // exactly the same asset and raw amount as the adverse transfer_call.
    fixture.mode(&harness, json!("Normal")).await?;
    let result = fixture.convert(&harness, 0).await?;
    resolution(&harness, &fixture, 0, &result, INPUT, false).await?;
    ensure!(
        storage(&harness, &market.market_id).await? == before_storage,
        "plain conversion mutated market storage"
    );
    ensure!(
        harness
            .get_supply_position(&market, &fixture.id)
            .await?
            .is_none(),
        "plain transfer credited converter supply"
    );
    let mut normal = before_balances;
    normal[0][0] -= INPUT;
    normal[0][1] += INPUT;
    normal[1][1] -= INPUT;
    normal[1][0] += INPUT;
    ensure!(
        fixture.balances(&harness).await? == normal,
        "plain market output balances mismatch"
    );
    fixture.mode(&harness, json!("DepositBack")).await?;
    let result = fixture.convert(&harness, 0).await?;
    let receipts = resolution(&harness, &fixture, 0, &result, INPUT, false).await?;
    ensure!(
        receipts
            .receipts
            .iter()
            .any(|receipt| receipt.executor_id == market.market_id
                && receipt.status == ReceiptStatus::Succeeded),
        "DepositBack never executed real market receiver"
    );
    let position = harness
        .get_supply_position(&market, &fixture.id)
        .await?
        .context("DepositBack did not create actual converter supply position")?;
    ensure!(
        u128::from(position.get_deposit().total()) == INPUT,
        "DepositBack credited wrong raw principal"
    );
    ensure!(
        harness
            .get_supply_position(&market, &fixture.sender.0)
            .await?
            .is_none(),
        "DepositBack credited market rather than actual output sender"
    );
    ensure!(
        storage(&harness, &market.market_id).await? != before_storage,
        "DepositBack failed to expose raw storage divergence"
    );
    normal[0][0] -= INPUT;
    normal[0][1] += INPUT;
    normal[1][1] -= INPUT;
    normal[1][0] += INPUT;
    ensure!(
        fixture.balances(&harness).await? == normal,
        "DepositBack real-token balances mismatch"
    );
    let settlement = fixture.settlement(&harness).await?;
    ensure!(
        settlement["mode"] == "DepositBack"
            && settlement["consumed"] == INPUT.to_string()
            && settlement["refunded"] == "0"
            && settlement["output"] == INPUT.to_string()
            && settlement["output_succeeded"] == true,
        "DepositBack callback misread actual output resolver"
    );
    Ok(())
}

#[tokio::test]
async fn requires_sandbox_converter_rejects_invalid_route_configuration() -> Result<()> {
    let harness = SandboxHarness::start_owned().await?;
    let owner = harness.create_user("route-owner").await?;
    let old = Asset::nep141("old.test.near".parse()?);
    let new = Asset::nep141("new.test.near".parse()?);
    let route = json!({"id":0,"old_asset":old,"new_asset":new});
    let too_many: Vec<_> = (0..33).map(|index| json!({"id":index,
        "old_asset":{"Nep245":{"contract_id":"old.test.near","token_id":format!("input:{index}")}},"new_asset":new})).collect();
    let invalid = [
        json!([]),
        json!(too_many),
        json!([{"id":0,"old_asset":old,"new_asset":old}]),
        json!([route, route]),
        json!([route,{"id":1,"old_asset":old,"new_asset":new}]),
        json!([{"id":0,"old_asset":{"Nep245":{"contract_id":"old.test.near","token_id":""}},"new_asset":new}]),
        json!([{"id":0,"old_asset":old,"new_asset":{"Nep245":{"contract_id":"new.test.near","token_id":"x".repeat(257)}}}]),
    ];
    for routes in invalid {
        let (id, signer) = harness
            .create_account_via_tx("invalid-converter", NearToken::from_near(100))
            .await?;
        let result = Contract::deploy(id)
            .use_code(
                templar_gateway_testing::wasm::mock_asset_converter()
                    .await
                    .to_vec(),
            )
            .with_init_call("new", json!({"owner_id":owner.0,"routes":routes}))?
            .with_signer(signer)
            .wait_until(TEST_FINALITY_POLICY.transaction_status())
            .send_to(&harness.network)
            .await?;
        ensure!(
            result.is_failure(),
            "invalid route configuration initialized a converter"
        );
        let failure = result
            .into_result()
            .err()
            .context("missing route constructor rejection")?
            .to_string();
        ensure!(
            failure.contains("Invalid route")
                || failure.contains("Route must change asset")
                || failure.contains("Duplicate route ID or input asset"),
            "constructor failed for unrelated reason: {failure}"
        );
    }
    // Inclusive boundaries: maximum route count and maximum UTF-8 token-ID
    // byte length are admitted with the same immutable profile representation.
    let routes: Vec<_> = (0..32).map(|index| json!({"id":index,
        "old_asset":{"Nep245":{"contract_id":"old.test.near","token_id":format!("{index:03}:{}","x".repeat(252))}},"new_asset":new})).collect();
    let id = deploy_converter(&harness, &owner, json!(routes)).await?;
    let profile = view(&harness, &id, "get_profile").await?;
    ensure!(
        profile["routes"] == json!(routes) && profile["schema"] == 1,
        "inclusive route bounds were not preserved"
    );
    Ok(())
}

#[rstest::rstest]
#[case::ft(false)]
#[case::mt(true)]
#[tokio::test]
async fn requires_sandbox_converter_refunds_real_input_when_output_transfer_fails(
    #[case] mt: bool,
) -> Result<()> {
    let harness = SandboxHarness::start_owned().await?;
    let fixture = Converter::new(&harness, mt, mt, None).await?;
    // Deliberately exhaust the funded destination inventory through the token's
    // real transfer entrypoint. The route stays normal and its profile unchanged.
    harness
        .execute(
            &ManagedAccountId(fixture.id.clone()),
            token::Transfer {
                token: (&fixture.assets[1]).into(),
                receiver_id: fixture.other.0.clone(),
                amount: INVENTORY.into(),
                memo: None,
            },
        )
        .await?;
    let before = fixture.balances(&harness).await?;
    let result = fixture.convert(&harness, 0).await?;
    ensure!(
        result.operation.status == OperationStatus::Succeeded,
        "old token resolver did not settle output failure"
    );
    let evidence = finalized_transaction(&harness, &fixture.sender.0, &result).await?;
    let zero = if mt { json!(["0"]) } else { json!("0") };
    ensure!(
        evidence.status == tx::Status::Succeeded
            && evidence.return_value == Some(tx::ReturnValue::Json(zero)),
        "native old-token resolver did not return zero consumption: {evidence:?}"
    );
    ensure!(
        evidence.failed_receipts == vec![fixture.assets[1].contract_id()],
        "failed output token receipt was hidden or misattributed: {evidence:?}"
    );
    ensure!(
        fixture.balances(&harness).await? == before,
        "failed output did not restore exact real sender/converter inventories"
    );
    let settlement = fixture.settlement(&harness).await?;
    ensure!(
        settlement
            == json!({
                "sequence":1,"route":0,"sender":fixture.sender.0,"recipient":fixture.sender.0,
                "old_asset":fixture.assets[0],"new_asset":fixture.assets[1],"mode":"Normal",
                "input":INPUT.to_string(),"consumed":"0","refunded":INPUT.to_string(),
                "output":"0","refund_is_requested":true,"output_succeeded":false
            }),
        "output-failure callback falsely reported settlement: {settlement}"
    );
    // This is a terminal refunded attempt, not evidence of successful conversion.
    // Replenishment permits a distinct later attempt without consuming route 1.
    fund(
        &harness,
        &ManagedAccountId(fixture.id.clone()),
        &fixture.assets[1],
        INPUT,
    )
    .await?;
    let replenished = fixture.balances(&harness).await?;
    let result = fixture.convert(&harness, 0).await?;
    resolution(&harness, &fixture, 0, &result, INPUT, false).await?;
    let mut expected = replenished;
    expected[0][0] -= INPUT;
    expected[0][1] += INPUT;
    expected[1][1] -= INPUT;
    expected[1][0] += INPUT;
    ensure!(
        fixture.balances(&harness).await? == expected,
        "later terminally refunded retry changed another route's inventories"
    );
    Ok(())
}
