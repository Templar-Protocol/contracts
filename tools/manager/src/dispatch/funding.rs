//! Will this deploy strand halfway?
//!
//! Reports, before anything is sent, whether every account that signs has the
//! balance to do so. The failure this prevents is discovering at step 4 that the
//! operator is short — with governance and the oracle already deployed and their
//! deposits spent.
//!
//! Reads the plan's *steps*, so it survives a hand-edited plan. That makes it
//! one of the few checks that still means something after an edit.

use std::collections::BTreeMap;

use anyhow::Context as _;
use near_account_id::AccountId;
use near_api::types::NearToken;
use templar_gateway_methods_spec::{account, chain};

use crate::commands::registry::STORAGE_AMOUNT_PER_BYTE;
use crate::context::CliContext;
use crate::spec::{
    check::{Check, Status},
    plan::PlanStep,
};

/// Headroom on the gas price, which can rise between planning and applying.
/// Prepaid gas is *reserved* in full at signing even though the remainder
/// refunds, so this is charged up front rather than at the actual burn.
const GAS_PRICE_SAFETY_NUMERATOR: u128 = 3;
const GAS_PRICE_SAFETY_DENOMINATOR: u128 = 2;

/// What one account must have on hand.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Requirement {
    pub deposits: NearToken,
    pub gas: NearToken,
    /// The running total's high-water mark.
    pub peak: NearToken,
    /// Cumulative requirement after each step this account signs, in order.
    /// Kept so a shortfall can name the step the deploy would actually stop at.
    cumulative: Vec<(usize, NearToken)>,
}

impl Requirement {
    fn charge(&mut self, index: usize, deposit: NearToken, gas: NearToken) {
        self.deposits = self.deposits.saturating_add(deposit);
        self.gas = self.gas.saturating_add(gas);
        let running = self.deposits.saturating_add(self.gas);
        self.peak = self.peak.max(running);
        self.cumulative.push((index, running));
    }

    /// The first step whose cumulative requirement exceeds what is available —
    /// where execution would actually stop.
    ///
    /// Not the step that reaches the peak: with nothing crediting an account the
    /// running total only rises, so the peak is always the *last* step and would
    /// name the same step however short the account is. Where it stops depends
    /// on the balance, so it cannot be computed until the balance is known.
    fn stops_at(&self, available: NearToken) -> Option<usize> {
        self.cumulative
            .iter()
            .find(|(_, running)| *running > available)
            .map(|(index, _)| *index)
    }
}

/// Walk the steps in order, charging each signer.
///
/// Sequential rather than a per-account sum, because the two differ as soon as a
/// plan has more than one signer and the balance moves between them. They do
/// *not* differ today: the artifact carries only function calls, and a
/// function-call deposit lands with the receiving contract rather than becoming
/// spendable balance for a later signer, so nothing credits an account and the
/// running total only rises. Crediting it would be optimistic in a check whose
/// whole value is being conservative — so the model is the honest one, and the
/// peak simply coincides with the total until an action exists that can move
/// balance between signers.
pub(super) fn simulate(
    steps: &[PlanStep],
    gas_price: NearToken,
) -> anyhow::Result<BTreeMap<AccountId, Requirement>> {
    let mut required: BTreeMap<AccountId, Requirement> = BTreeMap::new();

    for (index, step) in steps.iter().enumerate() {
        // Registered even with no calls, so a step cannot drop its signer out of
        // the report entirely — an unlisted account reads as "nothing to check".
        required.entry(step.signer_id.clone()).or_default();

        for call in &step.function_calls {
            // Fail closed. A step whose cost cannot be computed is an unknown
            // charge, and treating it as zero is how a check reports "funded"
            // for a plan it did not understand.
            let gas = u128::from(call.gas)
                .checked_mul(gas_price.as_yoctonear())
                .and_then(|cost| cost.checked_mul(GAS_PRICE_SAFETY_NUMERATOR))
                .map(|cost| cost / GAS_PRICE_SAFETY_DENOMINATOR)
                .with_context(|| {
                    format!(
                        "`{}` prepays {} gas, which overflows at the current gas \
                         price; its cost cannot be bounded",
                        step.label, call.gas
                    )
                })?;

            required.entry(step.signer_id.clone()).or_default().charge(
                index,
                call.deposit,
                NearToken::from_yoctonear(gas),
            );
        }
    }
    Ok(required)
}

/// Spendable balance: what is left after the storage stake is backed.
///
/// An account holding 44 NEAR with 44 staked for storage cannot spend it, and a
/// check against `amount` alone strands you on an account that looks funded.
///
/// `locked` is *not* subtracted. `amount` is already the liquid balance, and the
/// protocol backs the storage stake with `amount + locked` — so a validator's
/// stake absorbs its storage cost rather than adding to it. Subtracting `locked`
/// as well double-counts it and reports a well-funded staking account as short.
fn available(account: &account::GetResult) -> NearToken {
    let storage = STORAGE_AMOUNT_PER_BYTE.saturating_mul(u128::from(account.storage_usage));
    account
        .amount
        .saturating_sub(storage.saturating_sub(account.locked))
}

/// `funding.<account_id>` for every distinct signer in the plan.
///
/// One `view_account` per signer plus one block read, so it is cheap enough to
/// run at plan time *and* again at apply: balances drift, and the plan-time
/// answer is only as good as the moment it was taken.
pub(super) async fn checks(ctx: &CliContext, steps: &[PlanStep]) -> anyhow::Result<Vec<Check>> {
    // A failed read is a failed *check*, matching `targets_available`: aborting
    // the whole run on a transient RPC hiccup leaves no override, since the
    // sibling checks degrade gracefully and this one would not.
    let gas_price = match ctx.client.read(chain::GetBlock { block_hash: None }).await {
        Ok(block) => block.gas_price,
        Err(error) => {
            return Ok(vec![Check::new(
                "funding.gas_price",
                Status::failed(format!(
                    "could not read the current gas price ({error}), so no \
                     signer's cost can be bounded"
                )),
            )])
        }
    };

    let required = simulate(steps, gas_price)?;
    let mut checks = Vec::with_capacity(required.len());

    for (account_id, need) in required {
        let status = match ctx
            .client
            .read(account::Get {
                account_id: account_id.clone(),
            })
            .await
        {
            Ok(account) => verdict(&need, available(&account), steps),
            // A balance that cannot be read is not a balance that suffices.
            Err(error) => Status::failed(format!("could not read `{account_id}`: {error}")),
        };
        checks.push(Check::new(format!("funding.{account_id}"), status));
    }
    Ok(checks)
}

/// Compare what is needed against what is spendable.
///
/// Gross, not net: gas refunds are not credited, and the detail says so — a
/// conservative answer to "will I get stuck" is the correct one, but it must not
/// read as a miscalculation.
fn verdict(need: &Requirement, available: NearToken, steps: &[PlanStep]) -> Status {
    let detail = format!(
        "needs {} ({} deposits + {} prepaid gas, refunds not credited); \
         {available} spendable after storage staking",
        need.peak, need.deposits, need.gas,
    );

    let Some(stops_at) = need.stops_at(available) else {
        return Status::passed(detail);
    };
    let label = steps
        .get(stops_at)
        .map_or_else(|| format!("step {stops_at}"), |step| step.label.clone());

    Status::failed(format!(
        "{detail}. SHORT {} — would stop at `{label}`; top up to at least {}.",
        need.peak.saturating_sub(available),
        need.peak,
    ))
}

#[cfg(test)]
mod tests {
    use super::{available, simulate, Requirement};
    use crate::spec::plan::{PlanArgs, PlanFunctionCall, PlanStep};
    use near_api::types::NearToken;

    /// 1 yoctoNEAR per gas keeps the arithmetic readable: gas cost in yocto is
    /// then just the gas units, times the 1.5 safety factor.
    const UNIT_GAS_PRICE: NearToken = NearToken::from_yoctonear(1);

    fn step(signer: &str, deposit: NearToken, gas: u64) -> PlanStep {
        PlanStep {
            label: format!("{signer} step"),
            signer_id: signer.parse().expect("valid account"),
            receiver_id: "registry.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "deploy_market".to_owned(),
                args: PlanArgs::Json(serde_json::json!({})),
                gas,
                deposit,
            }],
        }
    }

    fn need(steps: &[PlanStep], signer: &str) -> Requirement {
        simulate(steps, UNIT_GAS_PRICE)
            .expect("simulate")
            .remove(&signer.parse::<near_account_id::AccountId>().expect("valid"))
            .expect("signer charged")
    }

    /// The whole point of walking the plan per signer: one account's total is
    /// not the plan's total. A check that summed everything against the
    /// operator would pass a plan a second signer cannot pay for.
    #[test]
    fn each_signer_is_charged_only_for_its_own_steps() {
        let steps = vec![
            step("alice.near", NearToken::from_near(3), 0),
            step("bob.near", NearToken::from_near(5), 0),
            step("alice.near", NearToken::from_near(1), 0),
        ];

        assert_eq!(need(&steps, "alice.near").peak, NearToken::from_near(4));
        assert_eq!(need(&steps, "bob.near").peak, NearToken::from_near(5));
    }

    /// Where a deploy stops depends on the *balance*, not on where the peak is.
    /// With nothing crediting an account the running total only rises, so the
    /// peak is always the last step and would name it however short the account
    /// is — which is a diagnostic that tells an operator nothing.
    #[test]
    fn the_stopping_point_follows_the_balance() {
        let steps = vec![
            step("alice.near", NearToken::from_near(3), 0),
            step("bob.near", NearToken::from_near(9), 0),
            step("alice.near", NearToken::from_near(4), 0),
        ];
        let alice = need(&steps, "alice.near");

        // Enough for step 0 (3 NEAR) but not for step 2's cumulative 7.
        assert_eq!(alice.stops_at(NearToken::from_near(5)), Some(2));
        // Not even the first charge.
        assert_eq!(alice.stops_at(NearToken::from_near(1)), Some(0));
        // Covers everything.
        assert_eq!(alice.stops_at(NearToken::from_near(7)), None);
    }

    /// `amount` is already liquid, and the protocol backs the storage stake with
    /// `amount + locked` — so a validator's stake absorbs its storage cost.
    /// Subtracting `locked` as well reports a well-funded staking account short.
    #[test]
    fn locked_stake_is_not_subtracted_twice() {
        let account = crate::spec::plan::testing::account(
            NearToken::from_near(20),
            NearToken::from_near(100),
            20_000,
        );

        assert_eq!(
            available(&account),
            NearToken::from_near(20),
            "the stake covers the 0.2 NEAR storage cost; the liquid 20 is intact"
        );
    }

    /// Prepaid gas is reserved in full at signing, and the price can rise
    /// between planning and applying — so it is charged with headroom, not at
    /// the expected burn.
    #[test]
    fn prepaid_gas_is_charged_with_headroom() {
        let steps = vec![step("alice.near", NearToken::from_yoctonear(0), 300)];
        let need = need(&steps, "alice.near");

        assert_eq!(need.gas, NearToken::from_yoctonear(450), "300 × 1.5");
        assert_eq!(need.peak, need.gas, "no deposit, so gas is the whole cost");
    }

    /// Staked storage cannot be spent. Comparing against `amount` alone strands
    /// you on an account that looks funded.
    #[test]
    fn storage_staking_is_not_spendable() {
        let account = crate::spec::plan::testing::account(
            NearToken::from_near(14),
            NearToken::from_near(0),
            10_000,
        );

        // 10_000 bytes × 1e19 yocto = 0.1 NEAR staked.
        assert_eq!(
            available(&account),
            NearToken::from_millinear(13_900),
            "storage staking and locked balance are both excluded"
        );
    }

    /// A cost that cannot be bounded is not a cost of zero — that is how a
    /// funding check reports "funded" for a plan it did not understand.
    #[test]
    fn an_unboundable_gas_cost_fails_closed() {
        let steps = vec![step("alice.near", NearToken::from_yoctonear(0), u64::MAX)];
        let error = simulate(&steps, NearToken::from_yoctonear(u128::MAX)).expect_err("overflow");

        assert!(
            format!("{error:#}").contains("cannot be bounded"),
            "{error:#}"
        );
    }
}
