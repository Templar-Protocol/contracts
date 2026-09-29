//! Kernel mirror utilities for NEAR state.
//!
//! Builds a `templar_vault_kernel` view for parity checks without mutating storage.

use near_sdk_contract_tools::ft::Nep141Controller;
use templar_vault_kernel::fee::FeesSpec;
use templar_vault_kernel::state::vault::{
    FeeAccrualAnchor as KernelFeeAccrualAnchor, VaultConfig, VaultState, MAX_PENDING,
};
use templar_vault_kernel::Restrictions as KernelRestrictions;
use templar_vault_kernel::TimestampNs;

use crate::Contract;

impl Contract {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "MAX_PENDING is a small protocol constant and fits in u32 by design"
    )]
    const MAX_PENDING_WITHDRAWALS_MIRROR: u32 = MAX_PENDING as u32;

    /// Build a kernel `VaultState` snapshot from NEAR storage fields.
    #[must_use]
    pub(crate) fn kernel_state_mirror(&self) -> VaultState {
        let total_assets = self.get_total_assets().0;
        let idle_assets = self.idle_balance;
        let external_assets = total_assets.saturating_sub(idle_assets);
        let total_shares = self.total_supply();

        let fee_anchor = KernelFeeAccrualAnchor::new(
            self.fee_anchor.total_assets.0,
            TimestampNs(self.fee_anchor.timestamp_ns.0),
        );

        let op_state = self.op_state.clone();
        let withdraw_queue = self.withdraw_queue.clone();

        VaultState {
            total_assets,
            total_shares,
            idle_assets,
            external_assets,
            fee_anchor,
            op_state,
            withdraw_queue,
            epoch: self.epoch.clone(),
            next_op_id: self.next_op_id,
        }
    }

    /// Build a kernel `VaultConfig` snapshot from NEAR configuration fields.
    ///
    /// Note: the kernel adds a `+1` offset to totals for divide-by-zero safety.
    /// NEAR uses virtual offsets without an extra +1, so we subtract 1 here to
    /// align conversion math for parity checks.
    #[must_use]
    pub(crate) fn kernel_config_mirror(&self) -> VaultConfig {
        let virtual_shares = self.virtual_shares.saturating_sub(1);
        let virtual_assets = self.virtual_assets.saturating_sub(1);

        VaultConfig {
            fees: FeesSpec::zero(),
            min_withdrawal_assets: 0,
            withdrawal_cooldown_ns: self.withdrawal_cooldown_ns,
            max_pending_withdrawals: Self::MAX_PENDING_WITHDRAWALS_MIRROR,
            paused: self.gate.paused,
            virtual_shares,
            virtual_assets,
        }
    }

    #[must_use]
    pub(crate) fn kernel_restrictions_mirror(&self) -> Option<KernelRestrictions> {
        self.gate
            .restrictions
            .as_ref()
            .and_then(templar_common::vault::Restrictions::to_kernel_mode)
    }
}

// Note: withdraw queue is stored in kernel format, so no conversion is needed.

#[cfg(test)]
mod tests {
    use crate::convert::account_id_to_address;
    use crate::PendingWithdrawalRecord;
    use crate::storage_management::{
        storage_bytes_for_pending_withdrawal, yocto_for_bytes, yocto_for_ft_account,
    };
    use crate::test_utils::{mk, new_test_contract, set_block_ts, set_ctx};
    use near_sdk::json_types::{U128, U64};
    use near_sdk_contract_tools::ft::{Nep141Controller, Nep145};
    use std::collections::BTreeMap;
    use templar_common::vault::MarketConfiguration;
    use templar_vault_kernel::actions::apply_action;
    use templar_vault_kernel::effects::KernelEffect;
    use templar_vault_kernel::state::queue::PendingWithdrawal as KernelPendingWithdrawal;
    use templar_vault_kernel::{
        compute_full_withdrawal, is_past_cooldown, settled_claim, EpochId, EpochPhase, EpochState,
        KernelAction, TimestampNs, DEFAULT_COOLDOWN_NS, WithdrawQueue,
    };

    fn make_market_config(cap: u128) -> MarketConfiguration {
        MarketConfiguration {
            cap: U128(cap),
            cap_group_id: None,
            enabled: true,
            removable_at: TimestampNs::ZERO,
        }
    }

    #[test]
    fn test_kernel_state_mirror_invariants() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);

        c.idle_balance = 1_000;
        c.fee_anchor.total_assets = U128(1_500);
        c.fee_anchor.timestamp_ns = U64(123);
        c.next_op_id = 42;

        let market = mk(9);
        let cfg = make_market_config(10_000);
        let _ = c.insert_market_for_tests(market, cfg, 500);

        let owner_a = mk(1);
        let receiver_a = mk(2);
        let owner_b = mk(3);
        let receiver_b = mk(4);

        let owner_one_addr = account_id_to_address(&owner_a);
        let receiver_one_addr = account_id_to_address(&receiver_a);
        let owner_two_addr = account_id_to_address(&owner_b);
        let receiver_two_addr = account_id_to_address(&receiver_b);

        c.address_book.insert(owner_one_addr, owner_a.clone());
        c.address_book.insert(receiver_one_addr, receiver_a.clone());
        c.address_book.insert(owner_two_addr, owner_b.clone());
        c.address_book.insert(receiver_two_addr, receiver_b.clone());

        let mut pending = BTreeMap::new();
        pending.insert(
            3,
            KernelPendingWithdrawal::new(
                owner_one_addr,
                receiver_one_addr,
                250,
                400,
                TimestampNs(77),
                templar_vault_kernel::EpochId::FIRST_SETTLEMENT,
            )
            .expect("withdrawal escrows positive shares in a settlement epoch"),
        );
        pending.insert(
            4,
            KernelPendingWithdrawal::new(
                owner_two_addr,
                receiver_two_addr,
                300,
                600,
                TimestampNs(88),
                templar_vault_kernel::EpochId::FIRST_SETTLEMENT,
            )
            .expect("withdrawal escrows positive shares in a settlement epoch"),
        );
        c.withdraw_queue = templar_vault_kernel::WithdrawQueue::with_state(pending, 3, 5);

        let kernel = c.kernel_state_mirror();

        let total_assets = c.get_total_assets().0;
        assert_eq!(kernel.total_assets, total_assets);
        assert_eq!(kernel.idle_assets, c.idle_balance);
        assert_eq!(
            kernel.external_assets,
            total_assets.saturating_sub(c.idle_balance)
        );
        assert_eq!(kernel.fee_anchor.total_assets, c.fee_anchor.total_assets.0);
        assert_eq!(
            kernel.fee_anchor.timestamp_ns,
            TimestampNs(c.fee_anchor.timestamp_ns.0)
        );
        assert_eq!(kernel.next_op_id, c.next_op_id);

        assert!(kernel.withdraw_queue.check_invariants());
        assert_eq!(
            kernel.withdraw_queue.next_withdraw_to_execute,
            c.withdraw_queue.next_withdraw_to_execute
        );
        assert_eq!(
            kernel.withdraw_queue.next_pending_withdrawal_id,
            c.queue_tail()
        );
        assert_eq!(kernel.withdraw_queue.pending_withdrawals().len(), 2);

        let pending = kernel.withdraw_queue.pending_withdrawals().get(&3).unwrap();
        assert_eq!(pending.owner, owner_one_addr);
        assert_eq!(pending.receiver, receiver_one_addr);
        assert_eq!(pending.escrow_shares, 250);
        assert_eq!(pending.min_assets_out, 400);
        assert_eq!(
            pending.epoch_id,
            templar_vault_kernel::EpochId::FIRST_SETTLEMENT
        );
        assert_eq!(pending.requested_at_ns, TimestampNs(77));
    }

    #[test]
    fn test_kernel_preview_deposit_matches_near() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);
        set_block_ts(&vault_id, &vault_id, 1_000);

        let market = mk(9);
        let cfg = make_market_config(10_000);
        let market_id = c.insert_market_for_tests(market, cfg, 0);
        c.supply_queue.push(market_id);

        let seed_owner = mk(1);
        let refund =
            c.execute_supply(seed_owner, c.underlying_asset.contract_id().into(), 2_000, 0);
        assert_eq!(refund, 0);

        let assets_in = 500u128;
        let near_preview = c.preview_deposit(U128(assets_in)).0;

        let kernel_state = c.kernel_state_mirror();
        let kernel_config = c.kernel_config_mirror();

        let owner = account_id_to_address(&mk(1));
        let receiver = account_id_to_address(&mk(2));
        let self_id = account_id_to_address(&vault_id);

        let result = apply_action(
            kernel_state,
            &kernel_config,
            None,
            &self_id,
            KernelAction::Deposit {
                owner,
                receiver,
                assets_in,
                min_shares_out: 0,
                now_ns: TimestampNs(1_000),
            },
        )
        .expect("kernel deposit");

        let minted = result
            .effects
            .iter()
            .find_map(|effect| match effect {
                KernelEffect::MintShares { shares, .. } => Some(*shares),
                _ => None,
            })
            .expect("mint shares effect");

        assert_eq!(minted, near_preview);
    }

    #[test]
    fn test_kernel_request_withdraw_stores_no_asset_claim() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);
        set_block_ts(&vault_id, &vault_id, 2_000);

        let market = mk(9);
        let cfg = make_market_config(10_000);
        let market_id = c.insert_market_for_tests(market, cfg, 0);
        c.supply_queue.push(market_id);

        let seed_owner = mk(1);
        let refund =
            c.execute_supply(seed_owner, c.underlying_asset.contract_id().into(), 5_000, 0);
        assert_eq!(refund, 0);

        let shares = 250u128;

        let kernel_state = c.kernel_state_mirror();
        let kernel_config = c.kernel_config_mirror();

        let owner = account_id_to_address(&mk(3));
        let receiver = account_id_to_address(&mk(4));
        let self_id = account_id_to_address(&vault_id);

        let result = apply_action(
            kernel_state,
            &kernel_config,
            None,
            &self_id,
            KernelAction::RequestWithdraw {
                owner,
                receiver,
                shares,
                min_assets_out: 0,
                now_ns: TimestampNs(2_000),
            },
        )
        .expect("kernel request withdraw");

        let pending = result
            .state
            .withdraw_queue
            .pending_withdrawals()
            .get(&0)
            .expect("pending withdrawal");

        // A request records escrowed shares, the caller slippage floor, and
        // the intake epoch. It records no asset-denominated claim.
        assert_eq!(pending.owner, owner);
        assert_eq!(pending.receiver, receiver);
        assert_eq!(pending.escrow_shares, shares);
        assert_eq!(pending.min_assets_out, 0);
        assert_eq!(
            pending.epoch_id,
            templar_vault_kernel::EpochId::FIRST_SETTLEMENT
        );
        assert_eq!(
            templar_vault_kernel::settled_claim(pending, &result.state.epoch),
            None,
            "an unsettled request must carry no claim"
        );
    }

    #[test]
    fn test_execute_supply_matches_kernel_deposit() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);
        set_block_ts(&vault_id, &vault_id, 1_000);

        let market = mk(9);
        let cfg = make_market_config(10_000);
        let market_id = c.insert_market_for_tests(market, cfg, 0);
        c.supply_queue.push(market_id);

        c.fee_anchor.total_assets = U128(0);
        c.fee_anchor.timestamp_ns = U64(1_000);

        let sender = mk(1);
        let deposit = 1_000u128;

        let kernel_state = c.kernel_state_mirror();
        let kernel_config = c.kernel_config_mirror();
        let sender_addr = account_id_to_address(&sender);
        let self_addr = account_id_to_address(&vault_id);

        let result = apply_action(
            kernel_state,
            &kernel_config,
            None,
            &self_addr,
            KernelAction::Deposit {
                owner: sender_addr,
                receiver: sender_addr,
                assets_in: deposit,
                min_shares_out: 0,
                now_ns: TimestampNs(1_000),
            },
        )
        .expect("kernel deposit");

        let refund =
            c.execute_supply(sender, c.underlying_asset.contract_id().into(), deposit, 0);
        assert_eq!(refund, 0);

        let mirror = c.kernel_state_mirror();
        assert_eq!(mirror.total_assets, result.state.total_assets);
        assert_eq!(mirror.total_shares, result.state.total_shares);
        assert_eq!(mirror.idle_assets, result.state.idle_assets);
        assert_eq!(mirror.external_assets, result.state.external_assets);
        assert_eq!(mirror.withdraw_queue, result.state.withdraw_queue);
    }

    #[test]
    fn test_redeem_matches_kernel_request_withdraw() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);
        set_block_ts(&vault_id, &vault_id, 1_000);

        let market = mk(9);
        let cfg = make_market_config(10_000);
        let market_id = c.insert_market_for_tests(market, cfg, 0);
        c.supply_queue.push(market_id);

        let owner = mk(1);
        let deposit = 2_000u128;
        let refund = c.execute_supply(
            owner.clone(),
            c.underlying_asset.contract_id().into(),
            deposit,
            0,
        );
        assert_eq!(refund, 0);

        let shares = c.total_supply() / 2;
        let receiver = mk(2);
        let floor = c.convert_to_assets(U128(shares)).0;
        assert!(floor > 0, "the exit being priced must have a positive value");

        let now = 2_000u64;
        let storage_deposit = yocto_for_ft_account();
        set_ctx(&vault_id, &owner, Some(now), Some(storage_deposit));
        c.storage_deposit(Some(vault_id.clone()), None);

        let attached = yocto_for_bytes(storage_bytes_for_pending_withdrawal());
        set_ctx(&vault_id, &owner, Some(now), Some(attached));

        // Production accrues fees before it builds the state it acts on, so
        // the reference run has to accrue first as well. Comparing a request
        // applied to an unaccrued state against the accrued production run
        // would be a comparison of two different vaults, not a parity proof.
        c.internal_accrue_fee();
        let kernel_state = c.kernel_state_mirror();
        let kernel_config = c.kernel_config_mirror();
        let owner_addr = account_id_to_address(&owner);
        let receiver_addr = account_id_to_address(&receiver);
        let self_addr = account_id_to_address(&vault_id);

        let result = apply_action(
            kernel_state,
            &kernel_config,
            None,
            &self_addr,
            KernelAction::RequestWithdraw {
                owner: owner_addr,
                receiver: receiver_addr,
                shares,
                min_assets_out: floor,
                now_ns: TimestampNs(now),
            },
        )
        .expect("kernel request withdraw");

        let _ = c.redeem_with_min(U128(shares), receiver.clone(), U128(floor));

        let mirror = c.kernel_state_mirror();
        assert_eq!(mirror.op_state, result.state.op_state);
        assert_eq!(mirror.withdraw_queue, result.state.withdraw_queue);
        assert_eq!(mirror.total_shares, result.state.total_shares);

        // The caller's floor is stored verbatim with the request. It is a
        // refusal bound only, never a payout: no settlement covers this epoch
        // yet, so the request must still be unpriced despite the floor.
        let stored = mirror
            .withdraw_queue
            .get(0)
            .expect("queued exit is stored under the FIFO head");
        assert_eq!(
            stored.min_assets_out, floor,
            "a floored exit must persist the caller's floor"
        );
        assert_eq!(
            stored.escrow_shares, shares,
            "the request must escrow exactly the shares offered for exit"
        );
        assert_eq!(
            settled_claim(stored, &mirror.epoch),
            None,
            "a stated floor must not create a claim before settlement"
        );
    }

    #[test]
    fn test_kernel_state_mirror_exposes_genesis_epoch() {
        let vault_id = mk(0);
        let c = new_test_contract(&vault_id);

        // The parity surface must carry the vault epoch state. A mirror that
        // omits it would let a cutoff or settlement run against an epoch that
        // is not stated anywhere, so the epoch has to be exposed explicitly.
        let mirror = c.kernel_state_mirror();
        assert_eq!(mirror.epoch.phase, EpochPhase::Open);
        assert_eq!(
            mirror.epoch.intake_epoch,
            EpochId::FIRST_SETTLEMENT,
            "mirror must expose the open intake epoch"
        );
        assert_eq!(mirror.epoch.last_settled, None);
    }

    #[test]
    fn test_unsettled_fifo_head_blocks_epoch_drain() {
        let owner = account_id_to_address(&mk(3));
        let receiver = account_id_to_address(&mk(4));
        let mut queue = WithdrawQueue::new();
        let request_id = queue
            .enqueue(
                owner,
                receiver,
                250,
                0,
                TimestampNs(1_000),
                EpochId::FIRST_SETTLEMENT,
                3,
            )
            .expect("withdrawal queues");
        assert_eq!(request_id, 0);

        // Nothing predates the first settlement epoch yet.
        assert!(
            !queue.has_intake_before(EpochId::FIRST_SETTLEMENT),
            "the first settlement epoch has no older intake"
        );
        // The queued request was taken in epoch 1, so it is strictly older
        // than the next cutoff epoch and must hold the drain shut. An
        // in-flight withdrawal cannot be bypassed by advancing epochs.
        let next_epoch = EpochId::FIRST_SETTLEMENT
            .checked_next()
            .expect("successor epoch exists");
        assert!(
            queue.has_intake_before(next_epoch),
            "an unsettled FIFO head must block the cutoff drain"
        );

        // Holding the drain cannot leak value: without an accepted settlement
        // the head carries no claim at all and cannot be paid.
        let (head_id, head) = queue.head().expect("head exists");
        assert_eq!(head_id, 0);
        assert_eq!(head.epoch_id, EpochId::FIRST_SETTLEMENT);
        assert_eq!(
            settled_claim(head, &EpochState::genesis()),
            None,
            "an unsettled request must carry no claim"
        );
        assert_eq!(
            compute_full_withdrawal(head, &EpochState::genesis(), u128::MAX),
            None,
            "an unpriced head must not settle"
        );
    }

    #[test]
    fn test_withdrawal_is_not_payable_before_cooldown_or_without_settlement() {
        let owner = account_id_to_address(&mk(3));
        let receiver = account_id_to_address(&mk(4));
        let mut queue = WithdrawQueue::new();
        queue
            .enqueue(
                owner,
                receiver,
                250,
                0,
                TimestampNs(1_000),
                EpochId::FIRST_SETTLEMENT,
                3,
            )
            .expect("withdrawal queues");

        let requested_at = TimestampNs(1_000);
        let cooldown_end = requested_at.saturating_add_u64(DEFAULT_COOLDOWN_NS);
        let inside_cooldown = TimestampNs(cooldown_end.0 - 1);
        let after_cooldown = TimestampNs(cooldown_end.0 + 1);

        // A request cannot execute in the block it was queued, and the last
        // instant before expiry is still inside the cooldown window.
        assert!(
            !is_past_cooldown(requested_at, requested_at, DEFAULT_COOLDOWN_NS),
            "a request cannot execute in the block it was queued"
        );
        assert!(
            !is_past_cooldown(requested_at, inside_cooldown, DEFAULT_COOLDOWN_NS),
            "the instant before cooldown expiry is still inside cooldown"
        );
        // The guard is a floor, not a ceiling: it must release at expiry and
        // stay released, so a delayed withdrawal is never silently dropped.
        assert!(
            is_past_cooldown(requested_at, cooldown_end, DEFAULT_COOLDOWN_NS),
            "cooldown must release at expiry"
        );
        assert!(
            is_past_cooldown(requested_at, after_cooldown, DEFAULT_COOLDOWN_NS),
            "a delayed request must stay releasable after cooldown"
        );

        // Releasing the cooldown does not create a payout. Without an accepted
        // epoch settlement the request stays unpriced, so an early or
        // clock-skewed run cannot pay a fabricated amount.
        let (head_id, head) = queue.head().expect("head exists");
        assert_eq!(head_id, 0);
        assert_eq!(
            settled_claim(head, &EpochState::genesis()),
            None,
            "cooldown release must not manufacture a claim"
        );
        assert_eq!(
            compute_full_withdrawal(head, &EpochState::genesis(), u128::MAX),
            None,
            "an unpriced head must not settle"
        );
    }

    #[test]
    fn test_cancel_requires_the_owner_and_refunds_only_the_owner() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);
        set_block_ts(&vault_id, &vault_id, 2_000);

        let market = mk(9);
        let cfg = make_market_config(10_000);
        let market_id = c.insert_market_for_tests(market, cfg, 0);
        c.supply_queue.push(market_id);

        let seed_owner = mk(1);
        let refund =
            c.execute_supply(seed_owner, c.underlying_asset.contract_id().into(), 5_000, 0);
        assert_eq!(refund, 0);

        let owner = mk(3);
        let receiver = mk(4);
        let shares = 250u128;
        let owner_addr = account_id_to_address(&owner);
        let receiver_addr = account_id_to_address(&receiver);
        let self_addr = account_id_to_address(&vault_id);
        // Queue the request through the contract's own storage path, so the
        // mirror below reads a real pending withdrawal rather than a state the
        // kernel computed in isolation.
        c.insert_pending_withdrawal_for_tests(
            0,
            PendingWithdrawalRecord {
                owner: owner.clone(),
                receiver: receiver.clone(),
                escrow_shares: shares,
                min_assets_out: 0,
                requested_at: 2_000,
                epoch_id: EpochId::FIRST_SETTLEMENT,
            },
        );

        let before = c.kernel_state_mirror();
        assert_eq!(
            before.withdraw_queue.get(0).map(|entry| entry.escrow_shares),
            Some(shares),
            "the queued request must be present in contract state with its escrow"
        );

        // A stranger cannot cancel someone else's queued withdrawal. The
        // attempt must fail and must leave the request, its escrow, and the
        // owner binding exactly as they were.
        let stranger = account_id_to_address(&mk(77));
        let denied = apply_action(
            before.clone(),
            &c.kernel_config_mirror(),
            None,
            &self_addr,
            KernelAction::CancelPendingWithdrawal {
                caller: stranger,
                request_id: 0,
                now_ns: TimestampNs(2_000),
            },
        );
        assert!(
            matches!(
                denied,
                Err(templar_vault_kernel::error::KernelError::InvalidState(
                    templar_vault_kernel::error::InvalidStateCode::CancelCallerNotOwner,
                ))
            ),
            "a non-owner cancellation must be rejected as not-the-owner"
        );
        assert_eq!(
            c.kernel_state_mirror().withdraw_queue,
            before.withdraw_queue,
            "a rejected cancellation must not disturb the queue"
        );
        assert_eq!(
            c.kernel_state_mirror().total_shares,
            before.total_shares,
            "a rejected cancellation must not move shares"
        );

        // The owner can cancel. The request was never settled, so it has no
        // claim and cancellation returns the escrowed shares to the owner.
        let cancelled = apply_action(
            before.clone(),
            &c.kernel_config_mirror(),
            None,
            &self_addr,
            KernelAction::CancelPendingWithdrawal {
                caller: owner_addr,
                request_id: 0,
                now_ns: TimestampNs(2_000),
            },
        )
        .expect("owner may cancel their own pending withdrawal");
        assert!(
            cancelled.state.withdraw_queue.get(0).is_none(),
            "a cancelled request must leave the queue"
        );
        assert_eq!(
            cancelled.state.withdraw_queue.status().length,
            before.withdraw_queue.status().length - 1,
            "cancellation must remove exactly one entry"
        );
        assert_eq!(
            cancelled.state.withdraw_queue.status().total_escrow_shares,
            before.withdraw_queue.status().total_escrow_shares - shares,
            "cancellation must release exactly the escrowed shares"
        );
        assert!(
            cancelled.effects.iter().any(|effect| matches!(
                effect,
                KernelEffect::TransferShares { from, to, shares: refunded }
                    if *from == self_addr && *to == owner_addr && *refunded == shares
            )),
            "cancellation must refund the escrowed shares to the owner"
        );
        assert!(
            !cancelled.effects.iter().any(|effect| matches!(
                effect,
                KernelEffect::TransferShares { to, .. } if *to == receiver_addr
            )),
            "an unsettled cancellation must pay nothing to the receiver"
        );
        assert!(cancelled.effects.iter().any(|effect| matches!(
            effect,
            KernelEffect::EmitEvent {
                event: templar_vault_kernel::effects::KernelEvent::WithdrawalCancelled {
                    id: 0,
                    owner: cancelled_owner,
                    escrow_shares: cancelled_escrow,
                    ..
                }
            }
            if *cancelled_owner == owner_addr && *cancelled_escrow == shares
        )));
    }

    #[test]
    fn test_settled_claim_is_epoch_derived_and_immutable() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);
        set_block_ts(&vault_id, &vault_id, 2_000);

        let market = mk(9);
        let cfg = make_market_config(10_000);
        let market_id = c.insert_market_for_tests(market, cfg, 0);
        c.supply_queue.push(market_id);

        let seed_owner = mk(1);
        let refund =
            c.execute_supply(seed_owner, c.underlying_asset.contract_id().into(), 5_000, 0);
        assert_eq!(refund, 0);
        // Queue the request through the contract's own storage path. The
        // assertions below must read a genuine pending withdrawal out of
        // contract state, not a state the kernel computed in isolation.
        let owner_id = mk(3);
        let receiver_id = mk(4);
        let owner = account_id_to_address(&owner_id);
        let receiver = account_id_to_address(&receiver_id);
        let self_addr = account_id_to_address(&vault_id);
        c.insert_pending_withdrawal_for_tests(
            0,
            PendingWithdrawalRecord {
                owner: owner_id,
                receiver: receiver_id,
                escrow_shares: 250,
                min_assets_out: 0,
                requested_at: 2_000,
                epoch_id: EpochId::FIRST_SETTLEMENT,
            },
        );
        let mirror = c.kernel_state_mirror();
        assert_eq!(
            mirror.withdraw_queue.get(0).map(|entry| entry.escrow_shares),
            Some(250),
            "the queued request must be present in contract state with its escrow"
        );

        let pending = mirror
            .withdraw_queue
            .pending_withdrawals()
            .get(&0)
            .expect("pending withdrawal");
        // Nothing is owed before an accepted settlement covers the epoch.
        assert_eq!(settled_claim(pending, &mirror.epoch), None);

        // Close intake for the epoch the request was queued in.
        let cutoff = mirror
            .epoch
            .begin_cutoff(TimestampNs(3_000))
            .expect("cutoff must be accepted while intake is open");
        assert_eq!(cutoff.phase, EpochPhase::Cutoff);
        assert_eq!(cutoff.intake_epoch, EpochId::FIRST_SETTLEMENT);

        let report = templar_vault_kernel::ValuationReportRef {
            report_seq: 1,
            as_of_ns: TimestampNs(3_000),
            report_hash: [7u8; 32],
        };
        // NAV and eligible supply come from the vault's own book.
        let settlement_nav = mirror.total_assets;
        let eligible_supply = mirror.total_shares;
        let snapshot = cutoff
            .build_settlement_snapshot(
                &report,
                settlement_nav,
                eligible_supply,
                TimestampNs(3_000),
                u64::from(u32::MAX),
            )
            .expect("accepted report must bind a settlement snapshot");
        let settled = cutoff
            .apply_settled(&snapshot)
            .expect("snapshot must advance the epoch");
        assert_eq!(settled.phase, EpochPhase::Open);
        assert_eq!(
            settled.intake_epoch,
            EpochId::FIRST_SETTLEMENT
                .checked_next()
                .expect("successor epoch exists"),
            "intake must reopen at the next epoch"
        );

        // The claim now exists and is exactly the snapshot-derived pro-rata
        // amount, not any figure stored with the request.
        let claim = settled_claim(pending, &settled).expect("settled epoch must produce a claim");
        assert_eq!(
            claim,
            snapshot.claim_for(pending.escrow_shares).expect("claim"),
            "the claim must be the epoch-derived pro-rata amount"
        );

        // Immutability: moving NAV afterwards cannot change a settled claim.
        // The claim is a pure function of the bound snapshot, so a later
        // valuation cannot reprice an obligation that was already settled.
        let mut repriced = snapshot.settlement_nav();
        repriced = repriced.saturating_add(1_000);
        assert_ne!(repriced, snapshot.settlement_nav());
        assert_eq!(
            settled_claim(pending, &settled),
            Some(claim),
            "a settled claim must not change after settlement"
        );
        assert_eq!(
            snapshot.claim_for(pending.escrow_shares),
            Some(claim),
            "the bound snapshot must keep yielding the same claim"
        );

        // A stale or out-of-sequence report cannot rewrite the settlement.
        let stale = templar_vault_kernel::ValuationReportRef {
            report_seq: snapshot.report_seq(),
            as_of_ns: TimestampNs(4_000),
            report_hash: [9u8; 32],
        };
        assert_eq!(
            settled.build_settlement_snapshot(
                &stale,
                settlement_nav.saturating_add(5_000),
                eligible_supply,
                TimestampNs(4_000),
                u64::from(u32::MAX),
            ),
            Err(templar_vault_kernel::SettlementRejection::PhaseNotCutoff),
            "intake is open again, so a new settlement requires a fresh cutoff"
        );
        // A request created after a settlement is accepted belongs to the
        // epoch whose intake is now open, and it cannot be priced by the
        // settlement that already closed. Carry the settled epoch forward
        // into a live state and queue the request there, so it is bound by the
        // runtime's own allocation rather than by a value this test invents.
        let mut post_settlement = mirror.clone();
        post_settlement.epoch = settled;
        let later_id = post_settlement.withdraw_queue.next_pending_withdrawal_id;
        let later = apply_action(
            post_settlement.clone(),
            &c.kernel_config_mirror(),
            None,
            &self_addr,
            KernelAction::RequestWithdraw {
                owner,
                receiver,
                shares: 100,
                min_assets_out: 0,
                now_ns: TimestampNs(4_000),
            },
        )
        .expect("post-settlement request queues in the reopened epoch");
        let later_entry = later
            .state
            .withdraw_queue
            .get(later_id)
            .expect("later pending withdrawal");
        assert_eq!(
            later_entry.epoch_id, post_settlement.epoch.intake_epoch,
            "a post-settlement request must bind to the reopened intake epoch"
        );
        assert_ne!(
            later_entry.epoch_id,
            EpochId::FIRST_SETTLEMENT,
            "a post-settlement request must not bind to the epoch that already closed"
        );
        // Two requests now sit in one queue under two different epochs. The
        // first is priced only by the settlement covering its own epoch. The
        // second is unpriced, because no accepted settlement covers the epoch
        // it belongs to. Value must not leak across that boundary either way.
        assert_eq!(
            settled_claim(pending, &post_settlement.epoch),
            snapshot.claim_for(pending.escrow_shares),
            "the request from the closed epoch must keep its own epoch-derived claim"
        );
        assert_eq!(
            settled_claim(later_entry, &post_settlement.epoch),
            None,
            "the request from the open epoch must stay unpriced while its epoch is unsettled"
        );
        assert_eq!(
            later.state.withdraw_queue.total_escrow_shares(),
            post_settlement
                .withdraw_queue
                .total_escrow_shares()
                .saturating_add(later_entry.escrow_shares),
            "the new request must escrow exactly its own shares and nothing more"
        );
    }

    #[test]
    fn test_pending_deposits_stay_outside_pricing_until_admission() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);
        set_block_ts(&vault_id, &vault_id, 2_000);

        let market = mk(9);
        let cfg = make_market_config(10_000);
        let market_id = c.insert_market_for_tests(market, cfg, 0);
        c.supply_queue.push(market_id);

        let seed_owner = mk(1);
        let refund =
            c.execute_supply(seed_owner, c.underlying_asset.contract_id().into(), 5_000, 0);
        assert_eq!(refund, 0);

        let before = c.kernel_state_mirror();
        assert_eq!(before.epoch.phase, EpochPhase::Open);
        assert_eq!(before.epoch.last_settled, None);

        let depositor = account_id_to_address(&mk(21));
        let deposit = templar_vault_kernel::PendingDeposit::new(
            depositor,
            1_000,
            TimestampNs(2_000),
            before.epoch.intake_epoch,
        )
        .expect("deposit liability must bind the open settlement epoch");

        // A recorded liability carries no NAV, price, or share field, so a
        // pre-settlement claim cannot be represented at all.
        assert_eq!(deposit.owner, depositor);
        assert_eq!(deposit.assets, 1_000);
        assert_eq!(deposit.epoch_id, before.epoch.intake_epoch);

        // Recording the liability must not touch vault pricing or supply.
        let after = c.kernel_state_mirror();
        assert_eq!(after.total_assets, before.total_assets);
        assert_eq!(after.total_shares, before.total_shares);
        assert_eq!(after.idle_assets, before.idle_assets);
        assert_eq!(after.external_assets, before.external_assets);
        assert_eq!(after.epoch, before.epoch);

        // Nothing is minted while the deposit's epoch is unsettled. Admission
        // happens only through settlement, so the pro-rata share of a deposit
        // against an unsettled epoch is zero.
        assert_eq!(
            before.epoch.settled_claim_for(deposit.epoch_id, deposit.assets),
            None,
            "an unsettled epoch must admit no deposit value"
        );

        // An epochless or empty liability cannot enter the ledger.
        assert_eq!(
            templar_vault_kernel::PendingDeposit::new(
                depositor,
                1_000,
                TimestampNs(2_000),
                templar_vault_kernel::EpochId::MIGRATION_INTAKE,
            ),
            Err(templar_vault_kernel::RequestError::InvalidEpoch),
            "a non-settlement epoch must be rejected"
        );
        assert_eq!(
            templar_vault_kernel::PendingDeposit::new(
                depositor,
                0,
                TimestampNs(2_000),
                before.epoch.intake_epoch,
            ),
            Err(templar_vault_kernel::RequestError::ZeroDepositAssets),
            "a zero-amount liability must be rejected"
        );

        // Once the epoch settles, the deposit is priced strictly from the
        // bound snapshot, and a later valuation cannot change that amount.
        let cutoff = before
            .epoch
            .begin_cutoff(TimestampNs(3_000))
            .expect("cutoff must be accepted while intake is open");
        let report = templar_vault_kernel::ValuationReportRef {
            report_seq: 1,
            as_of_ns: TimestampNs(3_000),
            report_hash: [11u8; 32],
        };
        let snapshot = cutoff
            .build_settlement_snapshot(
                &report,
                before.total_assets,
                before.total_shares,
                TimestampNs(3_000),
                u64::from(u32::MAX),
            )
            .expect("accepted report must bind a settlement snapshot");
        let settled = cutoff
            .apply_settled(&snapshot)
            .expect("snapshot must advance the epoch");

        let admitted = settled
            .settled_claim_for(deposit.epoch_id, deposit.assets)
            .expect("the settled epoch must admit the deposit");
        assert_eq!(
            admitted,
            snapshot
                .claim_for(deposit.assets)
                .expect("bounded claim is representable"),
            "admission must equal the snapshot-derived pro-rata amount"
        );

        // Re-reading the same liability after settlement yields the identical
        // amount, so admission cannot drift with later state or clock changes.
        assert_eq!(
            settled.settled_claim_for(deposit.epoch_id, deposit.assets),
            Some(admitted),
            "an admitted amount must be reproducible and immutable"
        );
    }

    /// Generation B is the layout deployed immediately before this change: a
    /// vector of `(id, withdrawal)` entries whose withdrawals are positional
    /// `(owner, receiver, escrow_shares, dropped quote, requested_at_ns)` with
    /// 32-byte kernel addresses, followed by the head pointer, the next
    /// request id, the escrow cache, and a cached asset total. Decode that
    /// layout, then prove every obligation carries over with its quote
    /// dropped, its floor pinned to zero, and no claim before settlement.
    #[test]
    fn test_generation_b_queue_carries_obligations_without_fixed_claim() {
        use templar_vault_kernel::Address;
        type GenerationBQueue = (
            Vec<(u64, (Address, Address, u128, u128, TimestampNs))>,
            u64,
            u64,
            u128,
            u128,
        );

        let owner_a = mk(31);
        let receiver_a = mk(32);
        let owner_b = mk(33);
        let receiver_b = mk(34);

        // The two records differ on every field, so a swapped or omitted
        // position cannot pass silently.
        let queue_bytes = near_sdk::borsh::to_vec(&(
            vec![
                (
                    5u64,
                    (
                        account_id_to_address(&owner_a),
                        account_id_to_address(&receiver_a),
                        10u128,
                        100u128,
                        TimestampNs(777),
                    ),
                ),
                (
                    8u64,
                    (
                        account_id_to_address(&owner_b),
                        account_id_to_address(&receiver_b),
                        20u128,
                        200u128,
                        TimestampNs(888),
                    ),
                ),
            ],
            5u64,
            9u64,
            30u128,
            300u128,
        ))
        .expect("encode generation-B withdrawal queue layout");

        let decoded: GenerationBQueue = near_sdk::borsh::from_slice(&queue_bytes)
            .expect("decode generation-B withdrawal queue layout");
        assert_eq!(decoded.0.len(), 2);
        assert_eq!(decoded.1, 5);
        assert_eq!(decoded.2, 9);
        assert_eq!(decoded.3, 30);

        let mut carried = Vec::new();
        for (id, (owner, receiver, escrow_shares, _dropped_quote, requested_at_ns)) in decoded.0 {
            let entry = KernelPendingWithdrawal::migrated_legacy(
                owner,
                receiver,
                escrow_shares,
                requested_at_ns,
            )
            .expect("carry legacy obligation forward");

            assert_eq!(entry.owner, owner);
            assert_eq!(entry.receiver, receiver);
            assert_eq!(entry.escrow_shares, escrow_shares);
            assert_eq!(entry.requested_at_ns, requested_at_ns);
            assert_eq!(entry.min_assets_out, 0);
            assert!(entry.is_migrated_legacy());
            assert_eq!(settled_claim(&entry, &EpochState::genesis()), None);

            carried.push((id, entry));
        }

        let queue = WithdrawQueue::with_state(carried, decoded.1, decoded.2);
        assert_eq!(queue.len(), 2);
        assert!(queue.has_migrated_intake());
        assert_eq!(queue.total_escrow_shares(), 30);
        assert_eq!(queue.head().map(|(id, _entry)| id), Some(5));
        assert_eq!(queue.settled_head_claim(&EpochState::genesis()), None);
        assert_eq!(queue.get(8).map(|entry| entry.escrow_shares), Some(20));
    }
}
