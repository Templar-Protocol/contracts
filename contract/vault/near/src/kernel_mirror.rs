//! Kernel mirror utilities for NEAR state.
//!
//! Builds a `templar_vault_kernel` view for parity checks without mutating storage.

use near_sdk_contract_tools::ft::Nep141Controller;
use templar_vault_kernel::fee::FeesSpec;
use templar_vault_kernel::state::vault::{
    FeeAccrualAnchor as KernelFeeAccrualAnchor, VaultConfig, VaultState, MAX_PENDING,
};
use templar_vault_kernel::Restrictions as KernelRestrictions;

use crate::Contract;

impl Contract {
    /// Build a kernel `VaultState` snapshot from NEAR storage fields.
    #[must_use]
    pub(crate) fn kernel_state_mirror(&self) -> VaultState {
        let total_assets = self.get_total_assets().0;
        let idle_assets = self.idle_balance;
        let external_assets = total_assets.saturating_sub(idle_assets);
        let total_shares = self.total_supply();

        let fee_anchor = KernelFeeAccrualAnchor::new(
            self.fee_anchor.total_assets.0,
            self.fee_anchor.timestamp_ns.0,
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

        let paused = matches!(
            self.gate.restrictions,
            Some(templar_common::vault::Restrictions::Paused)
        );

        VaultConfig {
            fees: FeesSpec::zero(),
            min_withdrawal_assets: 0,
            withdrawal_cooldown_ns: self.withdrawal_cooldown_ns,
            max_pending_withdrawals: MAX_PENDING as u32,
            paused,
            virtual_shares,
            virtual_assets,
        }
    }

    #[must_use]
    pub(crate) fn kernel_restrictions_mirror(&self) -> Option<KernelRestrictions> {
        self.gate.restrictions.clone()
    }
}

// Note: withdraw queue is stored in kernel format, so no conversion is needed.

#[cfg(test)]
mod tests {
    use crate::convert::account_id_to_address;
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
    use templar_vault_kernel::KernelAction;

    fn make_market_config(cap: u128) -> MarketConfiguration {
        MarketConfiguration {
            cap: U128(cap),
            cap_group_id: None,
            enabled: true,
            removable_at: 0,
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

        let owner_a_addr = account_id_to_address(&owner_a);
        let receiver_a_addr = account_id_to_address(&receiver_a);
        let owner_b_addr = account_id_to_address(&owner_b);
        let receiver_b_addr = account_id_to_address(&receiver_b);

        c.address_book.insert(owner_a_addr, owner_a.clone());
        c.address_book.insert(receiver_a_addr, receiver_a.clone());
        c.address_book.insert(owner_b_addr, owner_b.clone());
        c.address_book.insert(receiver_b_addr, receiver_b.clone());

        let mut pending = BTreeMap::new();
        pending.insert(
            3,
            KernelPendingWithdrawal::new(owner_a_addr, receiver_a_addr, 250, 400, 77),
        );
        pending.insert(
            4,
            KernelPendingWithdrawal::new(owner_b_addr, receiver_b_addr, 300, 600, 88),
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
        assert_eq!(kernel.fee_anchor.timestamp_ns, c.fee_anchor.timestamp_ns.0);
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
        assert_eq!(kernel.withdraw_queue.pending_withdrawals.len(), 2);

        let pending = kernel.withdraw_queue.pending_withdrawals.get(&3).unwrap();
        assert_eq!(pending.owner, owner_a_addr);
        assert_eq!(pending.receiver, receiver_a_addr);
        assert_eq!(pending.escrow_shares, 250);
        assert_eq!(pending.expected_assets, 400);
        assert_eq!(pending.requested_at_ns, 77);
    }

    #[test]
    fn test_kernel_preview_deposit_matches_near() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);
        set_block_ts(&vault_id, &vault_id, 1_000);

        c.idle_balance = 2_000;
        c.fee_anchor.total_assets = U128(2_000);
        c.fee_anchor.timestamp_ns = U64(1_000);

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
                now_ns: 1_000,
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
    fn test_kernel_request_withdraw_expected_assets_matches_near() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);
        set_block_ts(&vault_id, &vault_id, 2_000);

        c.idle_balance = 5_000;
        c.fee_anchor.total_assets = U128(5_000);
        c.fee_anchor.timestamp_ns = U64(2_000);

        let shares = 250u128;
        let near_expected = c.convert_to_assets(U128(shares)).0;

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
                now_ns: 2_000,
            },
        )
        .expect("kernel request withdraw");

        let pending = result
            .state
            .withdraw_queue
            .pending_withdrawals
            .get(&0)
            .expect("pending withdrawal");

        let expected = KernelPendingWithdrawal {
            owner,
            receiver,
            escrow_shares: shares,
            expected_assets: near_expected,
            requested_at_ns: 2_000,
        };

        assert_eq!(*pending, expected);
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
                now_ns: 1_000,
            },
        )
        .expect("kernel deposit");

        let refund = c.execute_supply(sender, c.underlying_asset.contract_id().into(), deposit);
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
        );
        assert_eq!(refund, 0);

        let shares = c.total_supply() / 2;
        let receiver = mk(2);
        let expected_assets = c.convert_to_assets(U128(shares)).0;

        let now = 2_000u64;
        let storage_deposit = yocto_for_ft_account();
        set_ctx(&vault_id, &owner, Some(now), Some(storage_deposit));
        c.storage_deposit(Some(vault_id.clone()), None);

        let attached = yocto_for_bytes(storage_bytes_for_pending_withdrawal());
        set_ctx(&vault_id, &owner, Some(now), Some(attached));

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
                min_assets_out: expected_assets,
                now_ns: now,
            },
        )
        .expect("kernel request withdraw");

        let _ = c.redeem(U128(shares), receiver);

        let mirror = c.kernel_state_mirror();
        assert_eq!(mirror.op_state, result.state.op_state);
        assert_eq!(mirror.withdraw_queue, result.state.withdraw_queue);
        assert_eq!(mirror.total_shares, result.state.total_shares);
    }
}
