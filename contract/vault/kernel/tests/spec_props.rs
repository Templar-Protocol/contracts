use proptest::prelude::*;

use templar_vault_kernel::{
    apply_action, FeesSpec, KernelAction, VaultConfig, VaultState, WithdrawQueue,
};
#[cfg(feature = "action-sync-external")]
use templar_vault_kernel::{AllocatingState, OpState};

fn addr(tag: u8, index: u64) -> [u8; 32] {
    let mut address = [0u8; 32];
    address[0] = tag;
    address[1..9].copy_from_slice(&index.to_le_bytes());
    address
}

fn vault_addr() -> [u8; 32] {
    addr(0xAA, 0)
}

fn default_config() -> VaultConfig {
    VaultConfig {
        fees: FeesSpec::zero(),
        min_withdrawal_assets: 1,
        withdrawal_cooldown_ns: 0,
        max_pending_withdrawals: 1024,
        paused: false,
        virtual_shares: 0,
        virtual_assets: 0,
    }
}

proptest! {
    #[test]
    fn prop_deposit_updates_state(assets in 1u64..1_000_000) {
        let state = VaultState::new();
        let config = default_config();
        let result = apply_action(
            state,
            &config,
            None,
            &vault_addr(),
            KernelAction::Deposit {
                owner: addr(0x11, 1),
                receiver: addr(0x22, 2),
                assets_in: assets as u128,
                min_shares_out: 0,
                now_ns: 0,
            },
        )
        .unwrap();

        prop_assert_eq!(result.state.total_assets, assets as u128);
        prop_assert_eq!(result.state.idle_assets, assets as u128);
        prop_assert!(result.state.total_shares > 0);
        prop_assert!(result.state.check_invariant());
    }

    #[test]
    fn prop_withdraw_queue_fifo(n in 1u8..20) {
        let mut queue = WithdrawQueue::new();
        let mut ids = Vec::new();
        for i in 0..n {
            let id = queue
                .enqueue(addr(0x33, i as u64), addr(0x44, i as u64), 10, 10, i as u64, 1024)
                .unwrap();
            ids.push(id);
        }

        for expected in ids {
            let (id, _) = queue.dequeue().unwrap();
            prop_assert_eq!(id, expected);
        }
        prop_assert_eq!(queue.len(), 0);
    }
}

#[cfg(feature = "action-sync-external")]
proptest! {
    #[test]
    fn prop_sync_external_assets_updates_total(
        idle in 0u64..1_000_000,
        external in 0u64..1_000_000,
    ) {
        let mut state = VaultState::new();
        state.idle_assets = idle as u128;
        state.external_assets = 0;
        state.total_assets = idle as u128;
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 7,
            index: 0,
            remaining: 0,
            plan: vec![(0, 0)],
        });

        let config = default_config();
        let result = apply_action(
            state,
            &config,
            None,
            &vault_addr(),
            KernelAction::SyncExternalAssets {
                new_external_assets: external as u128,
                op_id: 7,
                now_ns: 0,
            },
        )
        .unwrap();

        prop_assert_eq!(result.state.external_assets, external as u128);
        prop_assert_eq!(result.state.total_assets, idle as u128 + external as u128);
        prop_assert!(result.state.check_invariant());
    }
}
