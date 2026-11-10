//! Test-only funded converter. This is not an ABI or admission rule for a
//! production bridge. Routes exchange exactly one stored raw unit for one raw
//! unit; they never scale positions or mint output inventory.

#![allow(
    clippy::needless_pass_by_value,
    reason = "NEAR ABI wrappers deserialize owned parameters"
)]

use near_sdk::{
    env, ext_contract, json_types::U128, near, serde_json, AccountId, Gas, NearToken,
    PanicOnDefault, PromiseOrValue, PromiseResult,
};
use templar_common::{
    asset::{BorrowAsset, FungibleAsset},
    market::DepositMsg,
};

// BorrowAsset is only an identity/amount phantom here, not a market role.
type Asset = FungibleAsset<BorrowAsset>;
const MAX_ROUTES: usize = 32;
const MAX_TOKEN_ID_BYTES: usize = 256;
const CALLBACK_GAS: Gas = Gas::from_tgas(12);
// Test-only receive-hook calls need enough gas for the actual market receiver
// and its token resolver, independently of the simple-transfer helper.
const DEPOSIT_BACK_FT_GAS: Gas = Gas::from_tgas(100);
const DEPOSIT_BACK_MT_GAS: Gas = Gas::from_tgas(150);

#[derive(Clone)]
#[near(serializers = [json, borsh])]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub id: u32,
    pub old_asset: Asset,
    pub new_asset: Asset,
}

/// Deliberately adverse modes are fixture controls, not alternate routes.
#[derive(Clone)]
#[near(serializers = [json, borsh])]
pub enum FailureMode {
    Normal,
    Panic,
    RefundAll,
    RefundPartial,
    MissingOutput,
    WrongRecipient(AccountId),
    /// Supply is only meaningful for a route to the market's borrow asset.
    DepositBack,
    /// Deliver real inventory of a different token, not merely a misleading log.
    WrongOutputToken(Asset),
}

#[near(serializers = [json])]
#[serde(deny_unknown_fields)]
struct ConversionMsg {
    route: u32,
}

#[near(serializers = [json])]
pub struct Profile {
    pub schema: u32,
    pub profile: String,
    pub settlement_rule: String,
    pub requires_registration: bool,
    pub routes: Vec<Route>,
}

/// Input refund fields are requests to the OLD token resolver, not proof that
/// the resolver subsequently restored the sender's balance. Receipt graphs and
/// token balances remain the authoritative settlement evidence.
#[derive(Clone)]
#[near(serializers = [json, borsh])]
pub struct Settlement {
    pub sequence: u64,
    pub route: u32,
    pub sender: AccountId,
    pub recipient: AccountId,
    pub old_asset: Asset,
    pub new_asset: Asset,
    pub mode: FailureMode,
    pub input: U128,
    pub consumed: U128,
    pub refunded: U128,
    pub output: U128,
    pub refund_is_requested: bool,
    pub output_succeeded: bool,
}

#[near(serializers = [json])]
pub struct OutputContext {
    pub settlement: Settlement,
    pub requested_output: U128,
    pub deposit_back: bool,
}

#[allow(
    dead_code,
    reason = "ext_contract generates the used callback call builders"
)]
#[ext_contract(ext_self)]
trait OutputCallbacks {
    fn resolve_ft(&mut self, context: OutputContext) -> U128;
    fn resolve_mt(&mut self, context: OutputContext) -> Vec<U128>;
}

// Only the fault-injection mode needs receiver hooks. The shared asset helper's
// transfer_call currently budgets simple-transfer gas; do not change production
// contracts to exercise this test. These are the established token ABIs.
#[allow(
    dead_code,
    reason = "ext_contract generates the used deposit-back call builders"
)]
#[ext_contract(ext_deposit_back)]
trait DepositBackCalls {
    fn ft_transfer_call(
        &mut self,
        receiver_id: AccountId,
        amount: U128,
        memo: Option<String>,
        msg: String,
    ) -> U128;
    fn mt_transfer_call(
        &mut self,
        receiver_id: AccountId,
        token_id: String,
        amount: U128,
        msg: String,
    ) -> Vec<U128>;
}

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    owner_id: AccountId,
    routes: Vec<Route>,
    failure_mode: FailureMode,
    sequence: u64,
    // Bounded evidence: concurrent requests carry independent callback context;
    // this is the last completed callback, not an in-flight accounting map.
    last_settlement: Option<Settlement>,
}

#[near]
impl Contract {
    #[init]
    pub fn new(owner_id: AccountId, routes: Vec<Route>) -> Self {
        assert!(
            !routes.is_empty() && routes.len() <= MAX_ROUTES,
            "Invalid route count"
        );
        for (index, route) in routes.iter().enumerate() {
            assert!(
                route.old_asset != route.new_asset,
                "Route must change asset"
            );
            for asset in [&route.old_asset, &route.new_asset] {
                if let Some(token_id) = asset.nep245_token_id() {
                    assert!(
                        !token_id.is_empty() && token_id.len() <= MAX_TOKEN_ID_BYTES,
                        "Invalid route token ID"
                    );
                }
            }
            assert!(
                routes[..index].iter().all(|previous| {
                    previous.id != route.id && previous.old_asset != route.old_asset
                }),
                "Duplicate route ID or input asset"
            );
        }
        Self {
            owner_id,
            routes,
            failure_mode: FailureMode::Normal,
            sequence: 0,
            last_settlement: None,
        }
    }

    pub fn get_profile(&self) -> Profile {
        Profile {
            schema: 1,
            profile: "local-one-to-one".to_owned(),
            settlement_rule: "one-to-one-raw".to_owned(),
            requires_registration: false,
            routes: self.routes.clone(),
        }
    }

    pub fn set_failure_mode(&mut self, mode: FailureMode) {
        assert_eq!(env::predecessor_account_id(), self.owner_id, "Owner only");
        self.failure_mode = mode;
    }

    pub fn get_last_settlement(&self) -> Option<Settlement> {
        self.last_settlement.clone()
    }

    pub fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        let old_asset = Asset::nep141(env::predecessor_account_id());
        self.convert(sender_id, old_asset, amount, &msg, false)
    }

    /// Pinned NEP-245 receiver ABI: one input, directly owned by the sender.
    pub fn mt_on_transfer(
        &mut self,
        sender_id: AccountId,
        previous_owner_ids: Vec<AccountId>,
        token_ids: Vec<String>,
        amounts: Vec<U128>,
        msg: String,
    ) -> PromiseOrValue<Vec<U128>> {
        assert!(
            previous_owner_ids.len() == 1 && previous_owner_ids.first() == Some(&sender_id),
            "Wrong previous owner"
        );
        let [token_id] = <[String; 1]>::try_from(token_ids)
            .unwrap_or_else(|_| env::panic_str("Exactly one MT input required"));
        assert_eq!(amounts.len(), 1, "Exactly one MT amount required");
        let old_asset = Asset::nep245(env::predecessor_account_id(), token_id);
        match self.convert(sender_id, old_asset, amounts[0], &msg, true) {
            PromiseOrValue::Promise(promise) => PromiseOrValue::Promise(promise),
            PromiseOrValue::Value(unused) => PromiseOrValue::Value(vec![unused]),
        }
    }

    #[private]
    pub fn resolve_ft(&mut self, context: OutputContext) -> U128 {
        self.finish_output(context)
    }

    #[private]
    pub fn resolve_mt(&mut self, context: OutputContext) -> Vec<U128> {
        vec![self.finish_output(context)]
    }
}

impl Contract {
    fn convert(
        &mut self,
        sender: AccountId,
        old_asset: Asset,
        amount: U128,
        msg: &str,
        mt_input: bool,
    ) -> PromiseOrValue<U128> {
        assert!(amount.0 > 0, "Positive input required");
        let message: ConversionMsg =
            serde_json::from_str(msg).unwrap_or_else(|_| env::panic_str("Expected {route:ID}"));
        let route = self
            .routes
            .iter()
            .find(|route| route.id == message.route)
            .unwrap_or_else(|| env::panic_str("Unknown route"));
        assert_eq!(route.old_asset, old_asset, "Wrong input contract or token");
        assert!(
            !matches!(self.failure_mode, FailureMode::Panic),
            "Injected converter panic"
        );
        self.sequence = self
            .sequence
            .checked_add(1)
            .unwrap_or_else(|| env::panic_str("Sequence exhausted"));
        let recipient = match &self.failure_mode {
            FailureMode::WrongRecipient(account) => account.clone(),
            _ => sender.clone(),
        };
        let output = match self.failure_mode {
            FailureMode::RefundAll | FailureMode::MissingOutput => 0,
            FailureMode::RefundPartial => amount.0 / 2,
            _ => amount.0,
        };
        let deposit_back = matches!(self.failure_mode, FailureMode::DepositBack);
        let output_asset = match &self.failure_mode {
            FailureMode::WrongOutputToken(asset) => {
                assert_ne!(
                    *asset, route.new_asset,
                    "Fault token must differ from route output"
                );
                asset.clone()
            }
            _ => route.new_asset.clone(),
        };
        let settlement = Settlement {
            sequence: self.sequence,
            route: route.id,
            sender,
            recipient: recipient.clone(),
            old_asset,
            new_asset: output_asset,
            mode: self.failure_mode.clone(),
            input: amount,
            consumed: U128(0),
            refunded: amount,
            output: U128(0),
            refund_is_requested: true,
            output_succeeded: false,
        };
        if output == 0 {
            let mut settlement = settlement;
            if matches!(self.failure_mode, FailureMode::MissingOutput) {
                settlement.consumed = amount;
                settlement.refunded = U128(0);
            }
            let unused = settlement.refunded;
            self.record(settlement);
            return PromiseOrValue::Value(unused);
        }
        let promise = if deposit_back {
            let msg = serde_json::to_string(&DepositMsg::Supply)
                .unwrap_or_else(|_| env::panic_str("Supply serialization failed"));
            let token = ext_deposit_back::ext(settlement.new_asset.contract_id().to_owned())
                .with_attached_deposit(NearToken::from_yoctonear(1));
            if let Some(token_id) = settlement.new_asset.nep245_token_id() {
                token.with_static_gas(DEPOSIT_BACK_MT_GAS).mt_transfer_call(
                    recipient,
                    token_id.to_owned(),
                    U128(output),
                    msg,
                )
            } else {
                token.with_static_gas(DEPOSIT_BACK_FT_GAS).ft_transfer_call(
                    recipient,
                    U128(output),
                    None,
                    msg,
                )
            }
        } else {
            settlement.new_asset.transfer(recipient, output.into())
        };
        let context = OutputContext {
            settlement,
            requested_output: U128(output),
            deposit_back,
        };
        let callback = ext_self::ext(env::current_account_id()).with_static_gas(CALLBACK_GAS);
        PromiseOrValue::Promise(promise.then(if mt_input {
            callback.resolve_mt(context)
        } else {
            callback.resolve_ft(context)
        }))
    }

    fn finish_output(&mut self, context: OutputContext) -> U128 {
        assert_eq!(
            env::promise_results_count(),
            1,
            "Exactly one output result required"
        );
        #[allow(
            deprecated,
            reason = "callback must inspect the transfer's actual return bytes"
        )]
        let result = env::promise_result(0);
        let mut settlement = context.settlement;
        let delivered = match result {
            PromiseResult::Successful(bytes) if context.deposit_back => {
                // transfer_call returns the amount actually used after its token
                // resolver, not the receiver's requested unused amount.
                let used = if settlement.new_asset.nep245_token_id().is_some() {
                    serde_json::from_slice::<Vec<U128>>(&bytes)
                        .ok()
                        .filter(|amounts| amounts.len() == 1)
                        .map(|amounts| amounts[0].0)
                } else {
                    serde_json::from_slice::<U128>(&bytes)
                        .ok()
                        .map(|amount| amount.0)
                };
                used.filter(|used| *used <= context.requested_output.0)
                    .unwrap_or(0)
            }
            PromiseResult::Successful(_) => context.requested_output.0,
            PromiseResult::Failed => 0,
        };
        settlement.output_succeeded = delivered == context.requested_output.0;
        settlement.output = U128(delivered);
        settlement.consumed = U128(delivered);
        settlement.refunded = U128(settlement.input.0 - delivered);
        let unused = settlement.refunded;
        self.record(settlement);
        unused
    }

    fn record(&mut self, settlement: Settlement) {
        let event = serde_json::json!({
            "standard": "templar_test_asset_converter",
            "version": "1.0.0",
            "event": "settlement",
            "data": [&settlement],
        });
        env::log_str(&format!("EVENT_JSON:{event}"));
        self.last_settlement = Some(settlement);
    }
}

// Match common-linked mock contracts' Wasm getrandom binding. The converter
// itself does not generate keys or use randomness for settlement.
#[cfg(target_arch = "wasm32")]
mod custom_getrandom {
    #![allow(
        clippy::no_mangle_with_rust_abi,
        reason = "getrandom custom registration exports its required ABI"
    )]

    getrandom::register_custom_getrandom!(fill);

    #[allow(
        clippy::unnecessary_wraps,
        reason = "getrandom requires a fallible callback signature"
    )]
    fn fill(buf: &mut [u8]) -> Result<(), getrandom::Error> {
        let seed = near_sdk::env::random_seed_array();
        for chunk in buf.chunks_mut(seed.len()) {
            chunk.copy_from_slice(&seed[..chunk.len()]);
        }
        Ok(())
    }
}
