use super::*;

#[cold]
pub(crate) fn contract_error(msg: &'static str) -> RuntimeError {
    RuntimeError::contract_error(msg)
}

#[cold]
pub(crate) fn invalid_state_error(msg: &'static str) -> RuntimeError {
    RuntimeError::invalid_state(msg)
}

pub(crate) fn kernel_address_from_sdk(env: &Env, addr: &SdkAddress) -> Address {
    let strkey = addr.to_string();
    let strkey_bytes = strkey.to_bytes();
    let mut strkey_vec = vec![0u8; strkey_bytes.len() as usize];
    strkey_bytes.copy_into_slice(&mut strkey_vec);
    let mut raw = Vec::with_capacity(KERNEL_ADDRESS_DOMAIN.len() + strkey_vec.len());
    raw.extend_from_slice(KERNEL_ADDRESS_DOMAIN);
    raw.extend_from_slice(&strkey_vec);
    let bytes = Bytes::from_slice(env, &raw);
    env.crypto().sha256(&bytes).to_bytes().to_array()
}

fn is_contract_address(addr: &SdkAddress) -> bool {
    let bytes = addr.to_string().to_bytes();
    matches!(bytes.get(0), Some(b'C'))
}

pub(crate) fn require_contract_address(addr: &SdkAddress) -> Result<(), ContractError> {
    is_contract_address(addr)
        .then_some(())
        .ok_or(ContractError::InvalidInput)
}

fn serialize_fees_spec(fees: &FeesSpec) -> Result<Vec<u8>, RuntimeError> {
    postcard::to_allocvec(fees).map_err(|_| RuntimeError::storage_error("fees serialize failed"))
}

fn deserialize_fees_spec(bytes: &[u8]) -> Result<FeesSpec, RuntimeError> {
    postcard::from_bytes(bytes).map_err(|_| RuntimeError::storage_error("fees deserialize failed"))
}

pub(crate) fn load_fees_spec(env: &Env) -> Result<FeesSpec, RuntimeError> {
    let stored: Option<Bytes> = env.storage().instance().get(&VaultDataKey::FeesSpec);
    stored.map_or_else(
        || Ok(FeesSpec::zero()),
        |bytes| deserialize_fees_spec(&bytes.to_alloc_vec()),
    )
}

pub(crate) fn store_fees_spec(env: &Env, fees: &FeesSpec) -> Result<(), RuntimeError> {
    let bytes = serialize_fees_spec(fees)?;
    env.storage()
        .instance()
        .set(&VaultDataKey::FeesSpec, &Bytes::from_slice(env, &bytes));
    Ok(())
}

#[allow(deprecated)]
#[inline(never)]
pub(crate) fn emit_admin_event(env: &Env, action: soroban_sdk::Symbol) {
    env.events().publish((symbol_short!("admin"),), action);
}

#[allow(deprecated)]
#[inline(never)]
pub(crate) fn emit_alloc_event(env: &Env, market: u32, amount: i128, supply: bool) {
    env.events()
        .publish((symbol_short!("alloc"), supply), (market, amount));
}

pub(crate) fn require_allowed_adapter(
    env: &Env,
    adapter: &SdkAddress,
) -> Result<(), ContractError> {
    let allowed: Option<soroban_sdk::Vec<SdkAddress>> =
        env.storage().instance().get(&VaultDataKey::AllowedAdapters);
    if let Some(list) = allowed {
        for a in list.iter() {
            if a == *adapter {
                return Ok(());
            }
        }
        return Err(ContractError::Unauthorized);
    }
    Ok(())
}

pub(crate) fn adapter_for_market(env: &Env, market: u32) -> Result<SdkAddress, ContractError> {
    let adapters: Option<soroban_sdk::Vec<SdkAddress>> =
        env.storage().instance().get(&VaultDataKey::AllowedAdapters);
    let Some(adapters) = adapters else {
        return Err(ContractError::InvalidInput);
    };

    let mut queue_index: Option<u32> = None;
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        for (idx, target_id) in vault.supply_queue_targets().iter().enumerate() {
            if *target_id == market {
                queue_index = Some(
                    u32::try_from(idx)
                        .map_err(|_| RuntimeError::invalid_input("index overflow"))?,
                );
                return Ok(());
            }
        }
        Err(RuntimeError::invalid_input("market not in supply queue"))
    };
    with_contract_vault_contract_error(env, &mut call)?;

    let index = queue_index.ok_or(ContractError::InvalidInput)?;
    adapters.get(index).ok_or(ContractError::InvalidInput)
}

pub(crate) fn current_supply_queue_len(env: &Env) -> Result<u32, ContractError> {
    let mut len: u32 = 0;
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        let targets = vault.supply_queue_targets();
        len = u32::try_from(targets.len())
            .map_err(|_| RuntimeError::invalid_input("queue overflow"))?;
        Ok(())
    };
    with_contract_vault_contract_error(env, &mut call)?;
    Ok(len)
}

pub(crate) fn apply_fee_change(
    env: &Env,
    performance_fee_wad: i128,
    performance_recipient: SdkAddress,
    management_fee_wad: i128,
    management_recipient: SdkAddress,
    max_growth_rate_wad: Option<i128>,
) -> Result<(), ContractError> {
    if performance_fee_wad < 0 || management_fee_wad < 0 {
        return Err(ContractError::InvalidInput);
    }
    if performance_fee_wad as u128 > MAX_PERFORMANCE_FEE_WAD {
        return Err(ContractError::InvalidInput);
    }
    if management_fee_wad as u128 > MAX_MANAGEMENT_FEE_WAD {
        return Err(ContractError::InvalidInput);
    }

    let max_rate = match max_growth_rate_wad {
        Some(value) => {
            if value < 0 {
                return Err(ContractError::InvalidInput);
            }
            Some(Wad::from(value as u128))
        }
        None => None,
    };

    let performance_kernel = kernel_address_from_sdk(env, &performance_recipient);
    let management_kernel = kernel_address_from_sdk(env, &management_recipient);
    let fees = FeesSpec::new(
        FeeSlot::new(Wad::from(performance_fee_wad as u128), performance_kernel),
        FeeSlot::new(Wad::from(management_fee_wad as u128), management_kernel),
        max_rate,
    );

    runtime_to_contract(store_fees_spec(env, &fees))?;
    let storage = SorobanStorage::new(env);
    storage.save_address(&performance_kernel, &performance_recipient);
    storage.save_address(&management_kernel, &management_recipient);
    Ok(())
}

pub(crate) fn extend_storage_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
    let storage = SorobanStorage::new(env);
    storage.extend_ttl(DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
}

pub(crate) fn get_config_address(
    env: &Env,
    key: &soroban_sdk::Symbol,
) -> Result<SdkAddress, ContractError> {
    match env.storage().instance().get(key) {
        Some(address) => Ok(address),
        None => Err(ContractError::MissingConfig),
    }
}

#[inline]
fn require_config_address(
    env: &Env,
    key: &soroban_sdk::Symbol,
    msg: &'static str,
) -> Result<SdkAddress, RuntimeError> {
    get_config_address(env, key).map_err(|_| RuntimeError::storage_error(msg))
}

pub(crate) fn set_config_address(env: &Env, key: &soroban_sdk::Symbol, addr: &SdkAddress) {
    env.storage().instance().set(key, addr);
}

pub(crate) fn query_vault_field(env: &Env, f: fn(&VaultState) -> u128) -> i128 {
    let storage = SorobanStorage::new(env);
    match storage.load_state() {
        Ok(Some(versioned)) => to_i128(f(&versioned.state)).unwrap_or(0),
        Ok(None) | Err(_) => 0,
    }
}

pub(crate) fn query_vault_snapshot(env: &Env) -> (i128, i128, i128) {
    let storage = SorobanStorage::new(env);
    match storage.load_state() {
        Ok(Some(versioned)) => (
            to_i128(versioned.state.total_shares).unwrap_or(0),
            to_i128(versioned.state.idle_assets).unwrap_or(0),
            to_i128(versioned.state.external_assets).unwrap_or(0),
        ),
        Ok(None) | Err(_) => (0, 0, 0),
    }
}

pub(crate) fn sdk_string_to_alloc(
    value: soroban_sdk::String,
) -> Result<AllocString, ContractError> {
    let bytes = value.to_bytes();
    let mut raw = vec![0u8; bytes.len() as usize];
    bytes.copy_into_slice(&mut raw);
    AllocString::from_utf8(raw).map_err(|_| ContractError::InvalidInput)
}

pub(crate) fn migrate_legacy_paused(env: &Env) {
    let storage = SorobanStorage::new(env);

    if let Some(paused) = env
        .storage()
        .instance()
        .get::<_, bool>(&VaultDataKey::Paused)
    {
        storage.set_paused(paused);
        env.storage().instance().remove(&VaultDataKey::Paused);
        return;
    }

    if let Some(paused) = storage.take_legacy_paused() {
        storage.set_paused(paused);
    }
}

#[inline(never)]
pub(crate) fn load_vault_bootstrap<'a>(env: &'a Env) -> Result<VaultBootstrap<'a>, RuntimeError> {
    if migration_in_progress(env) {
        return Err(RuntimeError::invalid_state(
            "migration in progress - call migrate() first",
        ));
    }

    extend_storage_ttl(env);
    migrate_legacy_paused(env);
    let curator: SdkAddress =
        require_config_address(env, &VaultDataKey::Curator, "curator not set")?;
    let governance: SdkAddress =
        require_config_address(env, &VaultDataKey::Governance, "governance not set")?;
    let asset_token: SdkAddress =
        require_config_address(env, &VaultDataKey::AssetToken, "asset token not set")?;
    let share_token: SdkAddress =
        require_config_address(env, &VaultDataKey::ShareToken, "share token not set")?;

    let vault_sdk = env.current_contract_address();
    let vault_kernel = kernel_address_from_sdk(env, &vault_sdk);
    let curator_kernel = kernel_address_from_sdk(env, &curator);
    let governance_kernel = kernel_address_from_sdk(env, &governance);
    let asset_kernel = kernel_address_from_sdk(env, &asset_token);
    let share_kernel = kernel_address_from_sdk(env, &share_token);

    let mut config = ContractConfig::new(
        curator_kernel,
        vault_kernel,
        Vec::new(),
        Vec::new(),
        asset_kernel,
        share_kernel,
    );
    config = config.with_fees(load_fees_spec(env)?);

    let storage = SorobanStorage::new(env);
    let paused = storage.is_paused();
    let mut rbac_config = RbacConfig::with_curator(curator_kernel);
    rbac_config.add_role(governance_kernel, Role::Curator);

    let guard_addrs: Option<soroban_sdk::Vec<SdkAddress>> =
        env.storage().instance().get(&VaultDataKey::Guardians);
    if let Some(guardians) = guard_addrs {
        for g in guardians.iter() {
            rbac_config.add_role(kernel_address_from_sdk(env, &g), Role::Sentinel);
        }
    }

    let alloc_addrs: Option<soroban_sdk::Vec<SdkAddress>> =
        env.storage().instance().get(&VaultDataKey::Allocators);
    if let Some(allocators) = alloc_addrs {
        for a in allocators.iter() {
            rbac_config.add_role(kernel_address_from_sdk(env, &a), Role::Allocator);
        }
    }
    let sentinel: Option<SdkAddress> = env.storage().instance().get(&VaultDataKey::Sentinel);
    if let Some(sentinel_addr) = sentinel {
        rbac_config.add_role(kernel_address_from_sdk(env, &sentinel_addr), Role::Sentinel);
    }
    rbac_config.set_paused(paused);
    let auth = RbacAuth {
        config: rbac_config,
    };

    Ok(VaultBootstrap {
        config,
        storage,
        auth,
        asset_token,
        share_token,
    })
}

pub(crate) type ContractVaultCallback<'a> =
    dyn for<'b> FnMut(&mut ContractVault<'b>) -> Result<(), RuntimeError> + 'a;

#[inline(never)]
pub(crate) fn with_contract_vault(
    env: &Env,
    f: &mut ContractVaultCallback<'_>,
) -> Result<(), RuntimeError> {
    let bootstrap = load_vault_bootstrap(env)?;
    let share_adapter = ShareTokenAdapter::new(env, &bootstrap.share_token);
    let asset_adapter = SdkTokenAdapter::new(env, &bootstrap.asset_token);
    let interpreter = SorobanEffectInterpreter::new(env, &share_adapter, &asset_adapter);

    let mut vault = CuratorVault::new(
        bootstrap.config,
        bootstrap.storage,
        bootstrap.auth,
        interpreter,
    );
    vault.load_state()?;
    f(&mut vault)
}

#[inline]
pub(crate) fn transition_to_runtime<T, E>(result: Result<T, E>) -> Result<T, RuntimeError> {
    result.map_err(|_| RuntimeError::transition_error())
}

#[inline]
pub(crate) fn with_contract_vault_contract_error(
    env: &Env,
    f: &mut ContractVaultCallback<'_>,
) -> Result<(), ContractError> {
    runtime_to_contract(with_contract_vault(env, f))
}

#[inline]
pub(crate) fn require_signed(addr: &SdkAddress) {
    addr.require_auth();
}

#[inline]
pub(crate) fn migration_in_progress(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&MIGRATION_FLAG_KEY)
        .unwrap_or(false)
}

#[inline]
pub(crate) fn set_migration_in_progress(env: &Env, migrating: bool) {
    if migrating {
        env.storage().instance().set(&MIGRATION_FLAG_KEY, &true);
    } else {
        env.storage().instance().remove(&MIGRATION_FLAG_KEY);
    }
}

#[inline]
pub(crate) fn emit_pause_state_event(env: &Env, paused: bool) {
    let event = if paused {
        symbol_short!("paused")
    } else {
        symbol_short!("unpause")
    };
    env.events().publish((event,), ());
}

pub(crate) fn max_deposit_or_mint(env: &Env, use_shares: bool) -> Result<i128, ContractError> {
    let (state, config) = load_state_and_config(env)?;
    if state.op_state.is_idle() && !config.paused {
        let total = if use_shares {
            state.total_shares
        } else {
            state.total_assets
        };
        let remaining = u128::MAX.saturating_sub(total);
        Ok(remaining.min(i128::MAX as u128) as i128)
    } else {
        Ok(0)
    }
}

pub(crate) fn max_withdraw_or_redeem(
    env: &Env,
    owner: &SdkAddress,
    is_redeem: bool,
) -> Result<i128, ContractError> {
    let (state, config) = load_state_and_config(env)?;
    if !state.op_state.is_idle() {
        return Ok(0);
    }
    let owner_shares = share_balance(env, owner).max(0) as u128;
    let max = if is_redeem {
        let shares_from_idle = convert_to_shares(&state, &config, state.idle_assets);
        owner_shares.min(shares_from_idle)
    } else {
        let assets_from_shares = convert_to_assets(&state, &config, owner_shares);
        assets_from_shares.min(state.idle_assets)
    };
    Ok(i128::try_from(max).unwrap_or(0))
}

pub(crate) fn require_governance(env: &Env, caller: &SdkAddress) -> Result<(), ContractError> {
    require_signed(caller);
    let governance: SdkAddress = get_config_address(env, &VaultDataKey::Governance)?;
    if caller != &governance {
        return Err(ContractError::Unauthorized);
    }
    Ok(())
}

#[inline(never)]
pub(crate) fn governance_caller(env: &Env, caller: &SdkAddress) -> Result<Address, ContractError> {
    require_governance(env, caller)?;
    Ok(kernel_address_from_sdk(env, caller))
}
