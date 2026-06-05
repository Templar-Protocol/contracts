use alloc::{format, string::String as AllocString};

use soroban_sdk::testutils::{Address as _, Events as _};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Bytes, Env, IntoVal, Vec,
};
use templar_soroban_shared_types::{
    ExecuteWithdrawStatus, VaultCommand as WireVaultCommand,
    VaultCommandResult as WireVaultCommandResult,
};

use crate::{
    contract::{ProxyDataKey, Soroban4626ProxyContract},
    error::ContractError,
    ProxyPreviewView, ProxyViewResponse,
};

#[derive(Clone)]
#[contracttype]
enum MockVaultDataKey {
    Preview,
    RecordedPayloads,
    LastProxyViewCall,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[contracttype]
struct MockPreviewConfig {
    convert_to_shares: i128,
    convert_to_assets: i128,
    max_deposit: i128,
    max_mint: i128,
    max_withdraw: i128,
    max_redeem: i128,
    preview_mint_assets: i128,
    preview_withdraw_shares: i128,
}

impl MockPreviewConfig {
    const fn into_preview(self) -> ProxyPreviewView {
        (
            self.convert_to_shares,
            self.convert_to_assets,
            self.max_deposit,
            self.max_mint,
            self.max_withdraw,
            self.max_redeem,
            self.preview_mint_assets,
            self.preview_withdraw_shares,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
struct MockProxyViewCall {
    owner: Address,
    assets: i128,
    shares: i128,
}

#[contract]
struct MockVaultContract;

#[contractimpl]
impl MockVaultContract {
    pub fn set_preview(env: Env, preview: MockPreviewConfig) {
        env.storage()
            .instance()
            .set(&MockVaultDataKey::Preview, &preview);
    }

    pub fn recorded_payloads(env: Env) -> Vec<Bytes> {
        env.storage()
            .instance()
            .get(&MockVaultDataKey::RecordedPayloads)
            .unwrap_or(Vec::new(&env))
    }

    pub fn last_proxy_view_call(env: Env) -> Option<MockProxyViewCall> {
        env.storage()
            .instance()
            .get(&MockVaultDataKey::LastProxyViewCall)
    }

    pub fn execute(env: Env, payload: Bytes) -> Bytes {
        let mut payloads = Self::recorded_payloads(env.clone());
        payloads.push_back(payload.clone());
        env.storage()
            .instance()
            .set(&MockVaultDataKey::RecordedPayloads, &payloads);

        let command = WireVaultCommand::decode(&payload.to_alloc_vec()).expect("decode command");
        let result = match command {
            WireVaultCommand::DepositWithMin { .. } => WireVaultCommandResult::I128(1000),
            WireVaultCommand::RequestWithdraw { .. } => WireVaultCommandResult::U64(42),
            WireVaultCommand::ExecuteWithdraw { .. } => {
                WireVaultCommandResult::ExecuteWithdrawStatus(ExecuteWithdrawStatus {
                    op_state_before: 0,
                    op_state_after: 0,
                    assets_transferred: 0,
                    events_emitted: 0,
                })
            }
            _ => WireVaultCommandResult::Unit,
        };

        Bytes::from_slice(&env, &result.encode())
    }

    pub fn proxy_view(env: Env, owner: Address, assets: i128, shares: i128) -> ProxyViewResponse {
        env.storage().instance().set(
            &MockVaultDataKey::LastProxyViewCall,
            &MockProxyViewCall {
                owner: owner.clone(),
                assets,
                shares,
            },
        );

        let preview: MockPreviewConfig = env
            .storage()
            .instance()
            .get(&MockVaultDataKey::Preview)
            .unwrap_or_default();
        let self_address = env.current_contract_address();

        (
            (
                (
                    self_address.clone(),
                    self_address.clone(),
                    self_address.clone(),
                    self_address,
                ),
                (0, 0, false),
                (0, 0, 0, 0),
                (0, 0, 0, 0, 0),
            ),
            (Vec::new(&env), Vec::new(&env)),
            preview.into_preview(),
        )
    }
}

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum MockVaultError {
    Unauthorized = 1,
}

#[contract]
struct MockFailingVaultContract;

#[contractimpl]
impl MockFailingVaultContract {
    pub fn execute(_env: Env, _payload: Bytes) -> Result<Bytes, MockVaultError> {
        Err(MockVaultError::Unauthorized)
    }

    pub fn proxy_view(
        _env: Env,
        _owner: Address,
        _assets: i128,
        _shares: i128,
    ) -> Result<ProxyViewResponse, MockVaultError> {
        Err(MockVaultError::Unauthorized)
    }
}

struct Fixture {
    env: Env,
    proxy: Address,
    vault: Address,
    asset: Address,
    share: Address,
}

impl Fixture {
    fn new() -> Self {
        let env = Env::default();
        let proxy = env.register(Soroban4626ProxyContract, ());
        let vault = env.register(MockVaultContract, ());
        let share = Address::generate(&env);
        let asset = Address::generate(&env);
        Self {
            env,
            proxy,
            vault,
            asset,
            share,
        }
    }

    fn initialize(&self) -> Result<(), ContractError> {
        self.env.as_contract(&self.proxy, || {
            Soroban4626ProxyContract::initialize(
                self.env.clone(),
                self.vault.clone(),
                self.asset.clone(),
                self.share.clone(),
            )
        })
    }

    fn set_preview(&self, preview: MockPreviewConfig) {
        self.env.as_contract(&self.vault, || {
            MockVaultContract::set_preview(self.env.clone(), preview)
        });
    }

    fn recorded_payloads(&self) -> Vec<Bytes> {
        self.env.as_contract(&self.vault, || {
            MockVaultContract::recorded_payloads(self.env.clone())
        })
    }

    fn last_proxy_view_call(&self) -> MockProxyViewCall {
        self.env
            .as_contract(&self.vault, || {
                MockVaultContract::last_proxy_view_call(self.env.clone())
            })
            .expect("proxy_view call recorded")
    }

    fn proxy_events_debug(&self) -> AllocString {
        let events = self.env.events().all().filter_by_contract(&self.proxy);
        format!("{:?}", events.events())
    }
}

fn decode_command(payload: &Bytes) -> WireVaultCommand {
    WireVaultCommand::decode(&payload.to_alloc_vec()).expect("decode recorded payload")
}

fn address_wire(address: &Address) -> AllocString {
    AllocString::from_utf8(address.to_string().to_bytes().to_alloc_vec()).expect("valid address")
}

#[test]
fn test_initialize_success() {
    let fixture = Fixture::new();

    fixture.initialize().expect("initialize succeeds");

    fixture.env.as_contract(&fixture.proxy, || {
        let storage = fixture.env.storage().instance();
        assert_eq!(
            storage.get(&ProxyDataKey::VaultAddress),
            Some(fixture.vault.clone())
        );
        assert_eq!(
            storage.get(&ProxyDataKey::AssetToken),
            Some(fixture.asset.clone())
        );
        assert_eq!(
            storage.get(&ProxyDataKey::ShareToken),
            Some(fixture.share.clone())
        );
        assert_eq!(storage.get(&ProxyDataKey::Initialized), Some(true));
    });
}

#[test]
fn test_initialize_already_initialized() {
    let fixture = Fixture::new();

    fixture.initialize().expect("first initialize succeeds");
    let result = fixture.initialize();

    assert_eq!(result, Err(ContractError::AlreadyInitialized));
}

#[test]
fn test_extend_ttl_succeeds() {
    let fixture = Fixture::new();

    fixture.initialize().expect("initialize succeeds");
    let result = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::extend_ttl(fixture.env.clone())
    });

    assert_eq!(result, Ok(()));
}

#[test]
fn test_rejects_negative_amounts_before_vault_call() {
    let fixture = Fixture::new();
    let caller = Address::generate(&fixture.env);
    let owner = Address::generate(&fixture.env);
    let receiver = Address::generate(&fixture.env);

    fixture.initialize().expect("initialize succeeds");

    let deposit = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::deposit(fixture.env.clone(), caller.clone(), -1, receiver.clone())
    });
    let deposit_with_min = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::deposit_with_min(
            fixture.env.clone(),
            caller.clone(),
            1,
            receiver.clone(),
            -1,
        )
    });
    let mint = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::mint(fixture.env.clone(), caller.clone(), -1, receiver.clone())
    });
    let withdraw = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::withdraw(
            fixture.env.clone(),
            caller.clone(),
            -1,
            receiver.clone(),
            owner.clone(),
        )
    });
    let redeem = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::redeem(
            fixture.env.clone(),
            caller.clone(),
            -1,
            receiver.clone(),
            owner.clone(),
        )
    });
    let request = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::request_withdraw(
            fixture.env.clone(),
            owner.clone(),
            receiver,
            1,
            -1,
        )
    });

    assert_eq!(deposit, Err(ContractError::InvalidInput));
    assert_eq!(deposit_with_min, Err(ContractError::InvalidInput));
    assert_eq!(mint, Err(ContractError::InvalidInput));
    assert_eq!(withdraw, Err(ContractError::InvalidInput));
    assert_eq!(redeem, Err(ContractError::InvalidInput));
    assert_eq!(request, Err(ContractError::InvalidInput));
    assert_eq!(fixture.recorded_payloads().len(), 0);
}

#[test]
fn test_vault_error_codes_do_not_decode_as_proxy_errors() {
    let fixture = Fixture::new();
    let failing_vault = fixture.env.register(MockFailingVaultContract, ());
    let caller = Address::generate(&fixture.env);
    let receiver = Address::generate(&fixture.env);

    fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::initialize(
            fixture.env.clone(),
            failing_vault,
            fixture.asset.clone(),
            fixture.share.clone(),
        )
        .expect("initialize succeeds");
    });
    fixture.env.mock_all_auths();

    let result = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::deposit(fixture.env.clone(), caller, 100, receiver)
    });

    assert_eq!(result, Err(ContractError::VaultError));
}

#[test]
fn test_withdraw_rejects_negative_preview_shares() {
    let fixture = Fixture::new();
    let owner = Address::generate(&fixture.env);
    let receiver = Address::generate(&fixture.env);

    fixture.env.mock_all_auths();
    fixture.initialize().expect("initialize succeeds");
    fixture.set_preview(MockPreviewConfig {
        preview_withdraw_shares: -1,
        ..Default::default()
    });

    let result = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::withdraw(fixture.env.clone(), owner.clone(), 111, receiver, owner)
    });

    assert_eq!(result, Err(ContractError::InvalidInput));
    assert_eq!(fixture.recorded_payloads().len(), 0);
}

#[test]
fn test_deposit_command_serialization() {
    let fixture = Fixture::new();
    let caller = Address::generate(&fixture.env);
    let receiver = Address::generate(&fixture.env);

    fixture.env.mock_all_auths();
    fixture.initialize().expect("initialize succeeds");

    let minted = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::deposit(
            fixture.env.clone(),
            caller.clone(),
            250,
            receiver.clone(),
        )
    });

    assert_eq!(minted, Ok(1000));
    let payloads = fixture.recorded_payloads();
    assert_eq!(payloads.len(), 1);
    let command = decode_command(&payloads.get(0).expect("payload exists"));
    assert_eq!(
        command,
        WireVaultCommand::DepositWithMin {
            owner: address_wire(&caller),
            receiver: address_wire(&receiver),
            assets: 250,
            min_shares_out: 0,
        }
    );
}

#[test]
fn test_deposit_with_min_command_serialization() {
    let fixture = Fixture::new();
    let operator = Address::generate(&fixture.env);
    let receiver = Address::generate(&fixture.env);

    fixture.env.mock_all_auths();
    fixture.initialize().expect("initialize succeeds");

    let minted = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::deposit_with_min(
            fixture.env.clone(),
            operator.clone(),
            250,
            receiver.clone(),
            240,
        )
    });

    assert_eq!(minted, Ok(1000));
    let payloads = fixture.recorded_payloads();
    assert_eq!(payloads.len(), 1);
    let command = decode_command(&payloads.get(0).expect("payload exists"));
    assert_eq!(
        command,
        WireVaultCommand::DepositWithMin {
            owner: address_wire(&operator),
            receiver: address_wire(&receiver),
            assets: 250,
            min_shares_out: 240,
        }
    );
}

#[test]
fn test_deposit_emits_event() {
    let fixture = Fixture::new();
    let caller = Address::generate(&fixture.env);
    let receiver = Address::generate(&fixture.env);

    fixture.env.mock_all_auths();
    fixture.initialize().expect("initialize succeeds");

    let minted = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::deposit(
            fixture.env.clone(),
            caller.clone(),
            444,
            receiver.clone(),
        )
    });

    assert_eq!(minted, Ok(1000));
    let events = fixture
        .env
        .events()
        .all()
        .filter_by_contract(&fixture.proxy);
    assert_eq!(events.events().len(), 1);
    let rendered = fixture.proxy_events_debug();
    assert!(rendered.contains("Deposit"));
    assert!(rendered.contains("444"));
    assert!(rendered.contains("1000"));
}

#[test]
fn test_mint_uses_preview() {
    let fixture = Fixture::new();
    let caller = Address::generate(&fixture.env);
    let receiver = Address::generate(&fixture.env);

    fixture.env.mock_all_auths();
    fixture.initialize().expect("initialize succeeds");
    fixture.set_preview(MockPreviewConfig {
        preview_mint_assets: 333,
        ..Default::default()
    });

    let assets = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::mint(fixture.env.clone(), caller.clone(), 77, receiver.clone())
    });

    assert_eq!(assets, Ok(333));
    let events = fixture
        .env
        .events()
        .all()
        .filter_by_contract(&fixture.proxy);
    assert_eq!(events.events().len(), 1);
    let rendered = fixture.proxy_events_debug();
    assert!(rendered.contains("333"));
    assert!(rendered.contains("1000"));
    assert_eq!(
        fixture.last_proxy_view_call(),
        MockProxyViewCall {
            owner: caller,
            assets: 0,
            shares: 77,
        }
    );
    let payloads = fixture.recorded_payloads();
    assert_eq!(payloads.len(), 1);
    let command = decode_command(&payloads.get(0).expect("payload exists"));
    assert_eq!(
        command,
        WireVaultCommand::DepositWithMin {
            owner: address_wire(&fixture.last_proxy_view_call().owner),
            receiver: address_wire(&receiver),
            assets: 333,
            min_shares_out: 77,
        }
    );
}

#[test]
fn test_request_withdraw_returns_request_id() {
    let fixture = Fixture::new();
    let owner = Address::generate(&fixture.env);
    let receiver = Address::generate(&fixture.env);

    fixture.env.mock_all_auths();
    fixture.initialize().expect("initialize succeeds");

    let request_id = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::request_withdraw(
            fixture.env.clone(),
            owner.clone(),
            receiver.clone(),
            80,
            70,
        )
    });

    assert_eq!(request_id, Ok(42));
}

#[test]
fn test_execute_withdraw_returns_unit() {
    let fixture = Fixture::new();
    let caller = Address::generate(&fixture.env);

    fixture.env.mock_all_auths();
    fixture.initialize().expect("initialize succeeds");

    let result = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::execute_withdraw(fixture.env.clone(), caller)
    });

    assert_eq!(result, Ok(()));
}

#[test]
fn test_withdraw_queued_flow() {
    let fixture = Fixture::new();
    let owner = Address::generate(&fixture.env);
    let receiver = Address::generate(&fixture.env);

    fixture.env.mock_all_auths();
    fixture.initialize().expect("initialize succeeds");
    fixture.set_preview(MockPreviewConfig {
        preview_withdraw_shares: 75,
        ..Default::default()
    });

    let request_id = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::withdraw(
            fixture.env.clone(),
            owner.clone(),
            500,
            receiver.clone(),
            owner.clone(),
        )
    });

    assert_eq!(request_id, Ok(42));
    let payloads = fixture.recorded_payloads();
    assert_eq!(payloads.len(), 1);
    assert_eq!(
        decode_command(&payloads.get(0).expect("request payload exists")),
        WireVaultCommand::RequestWithdraw {
            owner: address_wire(&owner),
            receiver: address_wire(&receiver),
            shares: 75,
            min_assets_out: 500,
        }
    );
}

#[test]
fn test_withdraw_emits_redeem_request_event() {
    let fixture = Fixture::new();
    let owner = Address::generate(&fixture.env);
    let receiver = Address::generate(&fixture.env);

    fixture.env.mock_all_auths();
    fixture.initialize().expect("initialize succeeds");
    fixture.set_preview(MockPreviewConfig {
        preview_withdraw_shares: 61,
        ..Default::default()
    });

    let request_id = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::withdraw(
            fixture.env.clone(),
            owner.clone(),
            222,
            receiver.clone(),
            owner.clone(),
        )
    });

    assert_eq!(request_id, Ok(42));
    let events = fixture
        .env
        .events()
        .all()
        .filter_by_contract(&fixture.proxy);
    assert_eq!(events.events().len(), 1);
    let rendered = fixture.proxy_events_debug();
    assert!(rendered.contains("RedeemRequest"));
    assert!(rendered.contains("42"));
    assert!(rendered.contains("61"));
}

#[test]
fn test_redeem_queued_flow() {
    let fixture = Fixture::new();
    let owner = Address::generate(&fixture.env);
    let receiver = Address::generate(&fixture.env);

    fixture.env.mock_all_auths();
    fixture.initialize().expect("initialize succeeds");
    fixture.set_preview(MockPreviewConfig {
        convert_to_assets: 88,
        ..Default::default()
    });

    let request_id = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::redeem(
            fixture.env.clone(),
            owner.clone(),
            55,
            receiver.clone(),
            owner.clone(),
        )
    });

    assert_eq!(request_id, Ok(42));
    let payloads = fixture.recorded_payloads();
    assert_eq!(payloads.len(), 1);
    assert_eq!(
        decode_command(&payloads.get(0).expect("request payload exists")),
        WireVaultCommand::RequestWithdraw {
            owner: address_wire(&owner),
            receiver: address_wire(&receiver),
            shares: 55,
            min_assets_out: 88,
        }
    );
}

#[test]
fn test_convert_to_shares_queries_proxy_view() {
    let fixture = Fixture::new();

    fixture.initialize().expect("initialize succeeds");
    fixture.set_preview(MockPreviewConfig {
        convert_to_shares: 1234,
        ..Default::default()
    });

    let shares = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::convert_to_shares(fixture.env.clone(), 777)
    });

    assert_eq!(shares, Ok(1234));
    assert_eq!(
        fixture.last_proxy_view_call(),
        MockProxyViewCall {
            owner: fixture.proxy.clone(),
            assets: 777,
            shares: 0,
        }
    );
}

#[test]
fn test_max_withdraw_returns_zero_when_not_idle() {
    let fixture = Fixture::new();
    let owner = Address::generate(&fixture.env);

    fixture.initialize().expect("initialize succeeds");
    fixture.set_preview(MockPreviewConfig {
        max_withdraw: 0,
        ..Default::default()
    });

    let max_withdraw = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::max_withdraw(fixture.env.clone(), owner.clone())
    });

    assert_eq!(max_withdraw, Ok(0));
    assert_eq!(
        fixture.last_proxy_view_call(),
        MockProxyViewCall {
            owner,
            assets: 0,
            shares: 0,
        }
    );
}

#[test]
#[should_panic]
fn test_deposit_fails_without_auth() {
    let fixture = Fixture::new();
    let caller = Address::generate(&fixture.env);
    let receiver = Address::generate(&fixture.env);

    fixture.initialize().expect("initialize succeeds");
    fixture.env.mock_auths(&[]);

    fixture.env.invoke_contract::<i128>(
        &fixture.proxy,
        &soroban_sdk::Symbol::new(&fixture.env, "deposit"),
        (&caller, &100i128, &receiver).into_val(&fixture.env),
    );
}

#[test]
fn test_withdraw_rejects_delegated_operator() {
    let fixture = Fixture::new();
    let operator = Address::generate(&fixture.env);
    let owner = Address::generate(&fixture.env);
    let receiver = Address::generate(&fixture.env);

    fixture.env.mock_all_auths();
    fixture.initialize().expect("initialize succeeds");
    fixture.set_preview(MockPreviewConfig {
        preview_withdraw_shares: 50,
        ..Default::default()
    });

    let result = fixture.env.as_contract(&fixture.proxy, || {
        Soroban4626ProxyContract::withdraw(fixture.env.clone(), operator, 111, receiver, owner)
    });

    assert_eq!(result, Err(ContractError::InsufficientAllowance));
    assert_eq!(fixture.recorded_payloads().len(), 0);
}
