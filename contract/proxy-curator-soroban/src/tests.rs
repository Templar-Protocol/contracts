use soroban_sdk::testutils::Address as _;
use soroban_sdk::{contract, contractimpl, contracttype, Address, Bytes, Env, Vec};
use templar_soroban_governance::{
    GovernanceAction, GovernanceActionKind, GovernanceError, PendingProposal, TimelockKind,
    Timelocks,
};
use templar_soroban_runtime::ContractError as VaultContractError;
use templar_soroban_shared_types::{
    VaultCommand as WireVaultCommand, VaultCommandResult as WireVaultCommandResult,
    VAULT_ERR_ALREADY_INITIALIZED, VAULT_ERR_INVALID_INPUT,
};

use crate::{
    contract::{
        timelocks_from_kind_values, AllocationDelta, ProxyDataKey, SorobanCuratorProxyContract,
    },
    error::ContractError,
    CapGroupUpdate, CapGroupUpdateKey, Fees, Restrictions,
};

#[derive(Clone)]
#[contracttype]
enum MockVaultDataKey {
    RecordedPayloads,
}

#[contract]
struct MockVaultContract;

#[contractimpl]
impl MockVaultContract {
    pub fn recorded_payloads(env: Env) -> Vec<Bytes> {
        env.storage()
            .instance()
            .get(&MockVaultDataKey::RecordedPayloads)
            .unwrap_or(Vec::new(&env))
    }

    pub fn execute(env: Env, payload: Bytes) -> Bytes {
        let mut payloads = Self::recorded_payloads(env.clone());
        payloads.push_back(payload.clone());
        env.storage()
            .instance()
            .set(&MockVaultDataKey::RecordedPayloads, &payloads);

        let command = WireVaultCommand::decode(&payload.to_alloc_vec()).expect("decode command");
        let result = match command {
            WireVaultCommand::Allocate { .. } => WireVaultCommandResult::I128(123),
            WireVaultCommand::RefreshMarkets { .. } => WireVaultCommandResult::I128(456),
            WireVaultCommand::ResyncIdleBalance
            | WireVaultCommand::CancelMigration { .. }
            | WireVaultCommand::ExtendTtl => WireVaultCommandResult::Unit,
            _ => WireVaultCommandResult::Unit,
        };

        Bytes::from_slice(&env, &result.encode())
    }
}

#[derive(Clone)]
#[contracttype]
#[allow(clippy::enum_variant_names)]
enum MockGovernanceDataKey {
    LastSetCap,
    LastFees,
    LastRestrictions,
    LastCapGroupUpdate,
    LastAccept,
    LastRevoke,
    PendingIds,
    PendingProposal(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
struct MockSetCapCall {
    caller: Address,
    market_id: u32,
    new_cap: i128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
struct MockFeesCall {
    caller: Address,
    fees: Fees,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
struct MockRestrictionsCall {
    caller: Address,
    restrictions: Restrictions,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
struct MockCapGroupUpdateCall {
    caller: Address,
    update: CapGroupUpdate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
struct MockAcceptCall {
    caller: Address,
    proposal_id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
struct MockRevokeCall {
    caller: Address,
    proposal_id: u64,
}

#[contract]
struct MockGovernanceContract;

#[contractimpl]
impl MockGovernanceContract {
    pub fn submit_set_cap(
        env: Env,
        caller: Address,
        market_id: u32,
        new_cap: i128,
    ) -> Result<u64, GovernanceError> {
        env.storage().instance().set(
            &MockGovernanceDataKey::LastSetCap,
            &MockSetCapCall {
                caller,
                market_id,
                new_cap,
            },
        );
        Ok(77)
    }

    pub fn last_set_cap(env: Env) -> Option<MockSetCapCall> {
        env.storage()
            .instance()
            .get(&MockGovernanceDataKey::LastSetCap)
    }

    pub fn submit_set_fees(
        env: Env,
        caller: Address,
        performance_fee_wad: i128,
        performance_recipient: Address,
        management_fee_wad: i128,
        management_recipient: Address,
        max_growth_rate_wad: Option<i128>,
    ) -> Result<u64, GovernanceError> {
        let fees = Fees {
            performance_fee_wad,
            performance_recipient,
            management_fee_wad,
            management_recipient,
            max_growth_rate_wad,
        };
        env.storage().instance().set(
            &MockGovernanceDataKey::LastFees,
            &MockFeesCall { caller, fees },
        );
        Ok(88)
    }

    pub fn last_fees(env: Env) -> Option<MockFeesCall> {
        env.storage()
            .instance()
            .get(&MockGovernanceDataKey::LastFees)
    }

    pub fn submit_set_restrictions(
        env: Env,
        caller: Address,
        mode: u32,
        accounts: Vec<Address>,
    ) -> Result<u64, GovernanceError> {
        let restrictions = match mode {
            0 => Restrictions::None,
            1 => Restrictions::Blacklist(accounts),
            2 => Restrictions::Whitelist(accounts),
            _ => return Err(GovernanceError::InvalidInput),
        };
        env.storage().instance().set(
            &MockGovernanceDataKey::LastRestrictions,
            &MockRestrictionsCall {
                caller,
                restrictions,
            },
        );
        Ok(89)
    }

    pub fn last_restrictions(env: Env) -> Option<MockRestrictionsCall> {
        env.storage()
            .instance()
            .get(&MockGovernanceDataKey::LastRestrictions)
    }

    pub fn submit_set_group_member(
        env: Env,
        caller: Address,
        market_id: u32,
        cap_group_id: soroban_sdk::String,
    ) -> Result<u64, GovernanceError> {
        let update = CapGroupUpdate::SetMember(market_id, cap_group_id);
        env.storage().instance().set(
            &MockGovernanceDataKey::LastCapGroupUpdate,
            &MockCapGroupUpdateCall { caller, update },
        );
        Ok(90)
    }

    pub fn last_cap_group_update(env: Env) -> Option<MockCapGroupUpdateCall> {
        env.storage()
            .instance()
            .get(&MockGovernanceDataKey::LastCapGroupUpdate)
    }

    pub fn accept(env: Env, caller: Address, proposal_id: u64) -> Result<(), GovernanceError> {
        env.storage().instance().set(
            &MockGovernanceDataKey::LastAccept,
            &MockAcceptCall {
                caller,
                proposal_id,
            },
        );
        Ok(())
    }

    pub fn last_accept(env: Env) -> Option<MockAcceptCall> {
        env.storage()
            .instance()
            .get(&MockGovernanceDataKey::LastAccept)
    }

    pub fn revoke(env: Env, caller: Address, proposal_id: u64) -> Result<(), GovernanceError> {
        env.storage().instance().set(
            &MockGovernanceDataKey::LastRevoke,
            &MockRevokeCall {
                caller,
                proposal_id,
            },
        );
        Ok(())
    }

    pub fn last_revoke(env: Env) -> Option<MockRevokeCall> {
        env.storage()
            .instance()
            .get(&MockGovernanceDataKey::LastRevoke)
    }

    pub fn set_pending(env: Env, proposal: PendingProposal) {
        let mut ids = env
            .storage()
            .instance()
            .get(&MockGovernanceDataKey::PendingIds)
            .unwrap_or(Vec::new(&env));
        ids.push_back(proposal.id);
        env.storage()
            .instance()
            .set(&MockGovernanceDataKey::PendingIds, &ids);
        env.storage().instance().set(
            &MockGovernanceDataKey::PendingProposal(proposal.id),
            &proposal,
        );
    }

    pub fn pending(env: Env, proposal_id: u64) -> Result<PendingProposal, GovernanceError> {
        Ok(env
            .storage()
            .instance()
            .get(&MockGovernanceDataKey::PendingProposal(proposal_id))
            .unwrap_or(PendingProposal {
                id: proposal_id,
                action: GovernanceAction::SetPaused(true),
                valid_after_ns: 100,
            }))
    }

    pub fn pending_ids(env: Env) -> Vec<u64> {
        env.storage()
            .instance()
            .get(&MockGovernanceDataKey::PendingIds)
            .unwrap_or(Vec::from_array(&env, [1, 2, 3]))
    }

    pub fn timelock_ns(_env: Env, kind: TimelockKind) -> u64 {
        match kind {
            TimelockKind::Cap => 55,
            _ => 11,
        }
    }

    pub fn is_abdicated(_env: Env, kind: GovernanceActionKind) -> bool {
        kind == GovernanceActionKind::Skim
    }

    pub fn timelocks(_env: Env) -> Timelocks {
        Timelocks {
            admin_ns: 7,
            pause_ns: 7,
            curator_ns: 7,
            governance_ns: 7,
            supply_queue_ns: 7,
            fees_ns: 7,
            restrictions_ns: 7,
            sentinel_ns: 7,
            allocators_ns: 7,
            allowed_adapters_ns: 7,
            cap_ns: 7,
            market_removal_ns: 7,
            cap_group_ns: 7,
            skim_ns: 7,
            upgrade_ns: 7,
            migration_ns: 7,
            timelock_config_ns: 7,
            other_ns: 7,
        }
    }
}

struct Fixture {
    env: Env,
    proxy: Address,
    vault: Address,
    governance: Address,
}

impl Fixture {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        let proxy = env.register(SorobanCuratorProxyContract, ());
        let vault = env.register(MockVaultContract, ());
        let governance = env.register(MockGovernanceContract, ());
        Self {
            env,
            proxy,
            vault,
            governance,
        }
    }

    fn initialize(&self) -> Result<(), ContractError> {
        self.env.as_contract(&self.proxy, || {
            SorobanCuratorProxyContract::initialize(
                self.env.clone(),
                self.vault.clone(),
                self.governance.clone(),
            )
        })
    }

    fn recorded_payloads(&self) -> Vec<Bytes> {
        self.env.as_contract(&self.vault, || {
            MockVaultContract::recorded_payloads(self.env.clone())
        })
    }

    fn set_pending(&self, proposal: PendingProposal) {
        self.env.as_contract(&self.governance, || {
            MockGovernanceContract::set_pending(self.env.clone(), proposal)
        });
    }
}

fn decode_command(payload: &Bytes) -> WireVaultCommand {
    WireVaultCommand::decode(&payload.to_alloc_vec()).expect("decode recorded payload")
}

fn address_wire(address: &Address) -> alloc::string::String {
    alloc::string::String::from_utf8(address.to_string().to_bytes().to_alloc_vec())
        .expect("valid address")
}

#[test]
fn initialize_stores_target_contracts() {
    let fixture = Fixture::new();

    fixture.initialize().expect("initialize succeeds");

    fixture.env.as_contract(&fixture.proxy, || {
        let storage = fixture.env.storage().instance();
        assert_eq!(
            storage.get(&ProxyDataKey::VaultAddress),
            Some(fixture.vault.clone())
        );
        assert_eq!(
            storage.get(&ProxyDataKey::GovernanceAddress),
            Some(fixture.governance.clone())
        );
        assert_eq!(storage.get(&ProxyDataKey::Initialized), Some(true));
    });
}

#[test]
fn supply_market_encodes_allocate_command() {
    let fixture = Fixture::new();
    fixture.initialize().expect("initialize succeeds");
    let caller = Address::generate(&fixture.env);

    let result = fixture.env.as_contract(&fixture.proxy, || {
        SorobanCuratorProxyContract::allocate(
            fixture.env.clone(),
            caller.clone(),
            AllocationDelta::Supply(7, 500),
        )
    });

    assert_eq!(result, Ok(123));
    let payloads = fixture.recorded_payloads();
    assert_eq!(payloads.len(), 1);
    assert_eq!(
        decode_command(&payloads.get_unchecked(0)),
        WireVaultCommand::Allocate {
            caller: address_wire(&caller),
            market: 7,
            amount: 500,
            supply: true,
        }
    );
}

#[test]
fn refresh_markets_encodes_refresh_command() {
    let fixture = Fixture::new();
    fixture.initialize().expect("initialize succeeds");
    let caller = Address::generate(&fixture.env);
    let markets = Vec::from_array(&fixture.env, [1u32, 3u32]);

    let result = fixture.env.as_contract(&fixture.proxy, || {
        SorobanCuratorProxyContract::refresh_markets(
            fixture.env.clone(),
            caller.clone(),
            markets.clone(),
        )
    });

    assert_eq!(result, Ok(456));
    let payloads = fixture.recorded_payloads();
    assert_eq!(payloads.len(), 1);
    assert_eq!(
        decode_command(&payloads.get_unchecked(0)),
        WireVaultCommand::RefreshMarkets {
            caller: address_wire(&caller),
            markets: alloc::vec![1, 3],
        }
    );
}

#[test]
fn unit_vault_operations_encode_unit_commands() {
    let fixture = Fixture::new();
    fixture.initialize().expect("initialize succeeds");
    let caller = Address::generate(&fixture.env);

    fixture
        .env
        .as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::resync_idle_balance(fixture.env.clone())
        })
        .unwrap();
    fixture
        .env
        .as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::extend_vault_ttl(fixture.env.clone())
        })
        .unwrap();
    fixture
        .env
        .as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::cancel_migration(fixture.env.clone(), caller.clone())
        })
        .unwrap();

    let payloads = fixture.recorded_payloads();
    assert_eq!(payloads.len(), 3);
    assert_eq!(
        decode_command(&payloads.get_unchecked(0)),
        WireVaultCommand::ResyncIdleBalance
    );
    assert_eq!(
        decode_command(&payloads.get_unchecked(1)),
        WireVaultCommand::ExtendTtl
    );
    assert!(matches!(
        decode_command(&payloads.get_unchecked(2)),
        WireVaultCommand::CancelMigration { .. }
    ));
}

#[test]
fn governance_submit_forwards_typed_arguments() {
    let fixture = Fixture::new();
    fixture.initialize().expect("initialize succeeds");
    let caller = Address::generate(&fixture.env);

    let id = fixture.env.as_contract(&fixture.proxy, || {
        SorobanCuratorProxyContract::submit_cap(fixture.env.clone(), caller.clone(), 9, 1234)
    });

    assert_eq!(id, Ok(77));
    let recorded = fixture
        .env
        .as_contract(&fixture.governance, || {
            MockGovernanceContract::last_set_cap(fixture.env.clone())
        })
        .expect("set cap call recorded");
    assert_eq!(
        recorded,
        MockSetCapCall {
            caller,
            market_id: 9,
            new_cap: 1234,
        }
    );
}

#[test]
fn typed_governance_facade_forwards_domain_arguments() {
    let fixture = Fixture::new();
    fixture.initialize().expect("initialize succeeds");
    let admin = Address::generate(&fixture.env);
    let fee_recipient = Address::generate(&fixture.env);
    let restriction_account = Address::generate(&fixture.env);
    let group = soroban_sdk::String::from_str(&fixture.env, "senior");

    let fees = Fees {
        performance_fee_wad: 10,
        performance_recipient: fee_recipient.clone(),
        management_fee_wad: 20,
        management_recipient: fee_recipient,
        max_growth_rate_wad: Some(30),
    };
    assert_eq!(
        fixture.env.as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::set_fees(fixture.env.clone(), admin.clone(), fees.clone())
        }),
        Ok(88)
    );
    assert_eq!(
        fixture.env.as_contract(&fixture.governance, || {
            MockGovernanceContract::last_fees(fixture.env.clone())
        }),
        Some(MockFeesCall {
            caller: admin.clone(),
            fees
        })
    );

    let restrictions =
        Restrictions::Whitelist(Vec::from_array(&fixture.env, [restriction_account.clone()]));
    assert_eq!(
        fixture.env.as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::set_restrictions(
                fixture.env.clone(),
                admin.clone(),
                restrictions.clone(),
            )
        }),
        Ok(89)
    );
    assert_eq!(
        fixture.env.as_contract(&fixture.governance, || {
            MockGovernanceContract::last_restrictions(fixture.env.clone())
        }),
        Some(MockRestrictionsCall {
            caller: admin.clone(),
            restrictions,
        })
    );

    let update = CapGroupUpdate::SetMember(4, group);
    assert_eq!(
        fixture.env.as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::submit_cap_group_update(
                fixture.env.clone(),
                admin.clone(),
                update.clone(),
            )
        }),
        Ok(90)
    );
    assert_eq!(
        fixture.env.as_contract(&fixture.governance, || {
            MockGovernanceContract::last_cap_group_update(fixture.env.clone())
        }),
        Some(MockCapGroupUpdateCall {
            caller: admin,
            update,
        })
    );
}

#[test]
fn governance_lifecycle_and_views_forward() {
    let fixture = Fixture::new();
    fixture.initialize().expect("initialize succeeds");
    let caller = Address::generate(&fixture.env);

    fixture
        .env
        .as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::accept(fixture.env.clone(), caller.clone(), 44)
        })
        .unwrap();
    let recorded = fixture
        .env
        .as_contract(&fixture.governance, || {
            MockGovernanceContract::last_accept(fixture.env.clone())
        })
        .expect("accept call recorded");
    assert_eq!(
        recorded,
        MockAcceptCall {
            caller,
            proposal_id: 44,
        }
    );

    let pending = fixture
        .env
        .as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::pending(fixture.env.clone(), 12)
        })
        .unwrap();
    assert_eq!(pending.id, 12);
    assert_eq!(pending.valid_after_ns, 100);
    assert_eq!(
        fixture.env.as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::timelock_ns(fixture.env.clone(), TimelockKind::Cap)
        }),
        Ok(55)
    );
    assert_eq!(
        fixture.env.as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::is_abdicated(
                fixture.env.clone(),
                GovernanceActionKind::Skim,
            )
        }),
        Ok(true)
    );
}

#[test]
fn selector_accept_wrappers_resolve_exact_pending_proposal() {
    let fixture = Fixture::new();
    fixture.initialize().expect("initialize succeeds");
    let admin = Address::generate(&fixture.env);
    let group = soroban_sdk::String::from_str(&fixture.env, "senior");

    fixture.set_pending(PendingProposal {
        id: 40,
        action: GovernanceAction::SetCap(7, 100),
        valid_after_ns: 0,
    });
    fixture.set_pending(PendingProposal {
        id: 41,
        action: GovernanceAction::SetCap(9, 200),
        valid_after_ns: 0,
    });
    fixture.set_pending(PendingProposal {
        id: 42,
        action: GovernanceAction::SetGroupRelCap(group.clone(), 300),
        valid_after_ns: 0,
    });
    fixture.set_pending(PendingProposal {
        id: 43,
        action: GovernanceAction::SetTimelock(TimelockKind::Cap, 400),
        valid_after_ns: 0,
    });

    assert_eq!(
        fixture.env.as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::accept_cap(fixture.env.clone(), admin.clone(), 9)
        }),
        Ok(41)
    );
    assert_eq!(
        fixture.env.as_contract(&fixture.governance, || {
            MockGovernanceContract::last_accept(fixture.env.clone())
        }),
        Some(MockAcceptCall {
            caller: admin.clone(),
            proposal_id: 41,
        })
    );

    assert_eq!(
        fixture.env.as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::accept_cap_group_update(
                fixture.env.clone(),
                admin.clone(),
                CapGroupUpdateKey::RelativeCap(group.clone()),
            )
        }),
        Ok(42)
    );
    assert_eq!(
        fixture.env.as_contract(&fixture.governance, || {
            MockGovernanceContract::last_accept(fixture.env.clone())
        }),
        Some(MockAcceptCall {
            caller: admin.clone(),
            proposal_id: 42,
        })
    );

    assert_eq!(
        fixture.env.as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::accept_timelock(
                fixture.env.clone(),
                admin.clone(),
                Some(TimelockKind::Cap),
            )
        }),
        Ok(43)
    );
    assert_eq!(
        fixture.env.as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::accept_cap_group_update(
                fixture.env.clone(),
                admin,
                CapGroupUpdateKey::Cap(group),
            )
        }),
        Err(ContractError::GovernanceError)
    );
}

#[test]
fn selector_revoke_wrappers_resolve_exact_pending_proposal() {
    let fixture = Fixture::new();
    fixture.initialize().expect("initialize succeeds");
    let admin = Address::generate(&fixture.env);

    fixture.set_pending(PendingProposal {
        id: 50,
        action: GovernanceAction::RemoveMarket(3),
        valid_after_ns: 0,
    });
    fixture.set_pending(PendingProposal {
        id: 51,
        action: GovernanceAction::RemoveMarket(5),
        valid_after_ns: 0,
    });

    assert_eq!(
        fixture.env.as_contract(&fixture.proxy, || {
            SorobanCuratorProxyContract::revoke_pending_market_removal(
                fixture.env.clone(),
                admin.clone(),
                3,
            )
        }),
        Ok(1)
    );
    assert_eq!(
        fixture.env.as_contract(&fixture.governance, || {
            MockGovernanceContract::last_revoke(fixture.env.clone())
        }),
        Some(MockRevokeCall {
            caller: admin,
            proposal_id: 50,
        })
    );
}

#[test]
fn timelocks_from_scalar_kind_values_preserves_every_field() {
    let timelocks = timelocks_from_kind_values(|kind| match kind {
        TimelockKind::Admin => 0,
        TimelockKind::Pause => 1,
        TimelockKind::Curator => 2,
        TimelockKind::Governance => 3,
        TimelockKind::SupplyQueue => 4,
        TimelockKind::Fees => 5,
        TimelockKind::Restrictions => 6,
        TimelockKind::Sentinel => 8,
        TimelockKind::Allocators => 9,
        TimelockKind::AllowedAdapters => 10,
        TimelockKind::Cap => 11,
        TimelockKind::MarketRemoval => 12,
        TimelockKind::CapGroup => 13,
        TimelockKind::Skim => 14,
        TimelockKind::Upgrade => 15,
        TimelockKind::Migration => 16,
        TimelockKind::TimelockConfig => 17,
        TimelockKind::Other => 18,
    });

    assert_eq!(timelocks.admin_ns, 0);
    assert_eq!(timelocks.pause_ns, 1);
    assert_eq!(timelocks.curator_ns, 2);
    assert_eq!(timelocks.governance_ns, 3);
    assert_eq!(timelocks.supply_queue_ns, 4);
    assert_eq!(timelocks.fees_ns, 5);
    assert_eq!(timelocks.restrictions_ns, 6);
    assert_eq!(timelocks.sentinel_ns, 8);
    assert_eq!(timelocks.allocators_ns, 9);
    assert_eq!(timelocks.allowed_adapters_ns, 10);
    assert_eq!(timelocks.cap_ns, 11);
    assert_eq!(timelocks.market_removal_ns, 12);
    assert_eq!(timelocks.cap_group_ns, 13);
    assert_eq!(timelocks.skim_ns, 14);
    assert_eq!(timelocks.upgrade_ns, 15);
    assert_eq!(timelocks.migration_ns, 16);
    assert_eq!(timelocks.timelock_config_ns, 17);
    assert_eq!(timelocks.other_ns, 18);
}

#[test]
fn vault_error_code_mapping_matches_runtime_discriminants() {
    assert_eq!(
        VAULT_ERR_INVALID_INPUT,
        VaultContractError::InvalidInput as u32
    );
    assert_eq!(
        VAULT_ERR_ALREADY_INITIALIZED,
        VaultContractError::AlreadyInitialized as u32
    );
    assert_eq!(
        ContractError::from_vault_error_code(VAULT_ERR_INVALID_INPUT),
        ContractError::InvalidInput
    );
    assert_eq!(
        ContractError::from_vault_error_code(VAULT_ERR_ALREADY_INITIALIZED),
        ContractError::AlreadyInitialized
    );
    assert_eq!(
        ContractError::from_vault_error_code(u32::MAX),
        ContractError::VaultError
    );
}
