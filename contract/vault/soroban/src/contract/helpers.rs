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
    let strkey_bytes = addr.to_string().to_bytes().to_alloc_vec();
    let mut raw = Vec::with_capacity(KERNEL_ADDRESS_DOMAIN.len() + strkey_bytes.len());
    raw.extend_from_slice(KERNEL_ADDRESS_DOMAIN);
    raw.extend_from_slice(&strkey_bytes);
    let bytes = Bytes::from_slice(env, &raw);
    Address(env.crypto().sha256(&bytes).to_bytes().to_array())
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

fn allowed_adapters(env: &Env) -> Option<soroban_sdk::Vec<SdkAddress>> {
    env.storage().instance().get(&VaultDataKey::AllowedAdapters)
}

fn load_policy_state(env: &Env) -> Result<PolicyState, ContractError> {
    if migration_in_progress(env) {
        return Err(ContractError::InvalidState);
    }

    let storage = SorobanStorage::new(env);
    runtime_to_contract(storage.load_policy_state()).map(|state| state.unwrap_or_default())
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

pub(crate) fn load_virtual_offsets(env: &Env) -> (u128, u128) {
    let virtual_shares = env
        .storage()
        .instance()
        .get(&VaultDataKey::VirtualShares)
        .unwrap_or(0u128);
    let virtual_assets = env
        .storage()
        .instance()
        .get(&VaultDataKey::VirtualAssets)
        .unwrap_or(0u128);
    (virtual_shares, virtual_assets)
}

pub(crate) fn store_virtual_offsets(env: &Env, virtual_shares: u128, virtual_assets: u128) {
    env.storage()
        .instance()
        .set(&VaultDataKey::VirtualShares, &virtual_shares);
    env.storage()
        .instance()
        .set(&VaultDataKey::VirtualAssets, &virtual_assets);
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

pub(crate) fn adapter_for_market(env: &Env, market: u32) -> Result<SdkAddress, ContractError> {
    let adapters = allowed_adapters(env);
    let Some(adapters) = adapters else {
        return Err(ContractError::InvalidInput);
    };

    let policy_state = load_policy_state(env)?;
    for (idx, entry) in policy_state.supply_queue.entries.iter().enumerate() {
        if entry.target_id == market {
            let index = u32::try_from(idx).map_err(|_| ContractError::InvalidInput)?;
            return adapters.get(index).ok_or(ContractError::InvalidInput);
        }
    }

    Err(ContractError::InvalidInput)
}

pub(crate) fn current_supply_queue_len(env: &Env) -> Result<u32, ContractError> {
    let policy_state = load_policy_state(env)?;
    u32::try_from(policy_state.supply_queue.len()).map_err(|_| ContractError::InvalidInput)
}

fn require_non_negative_bounded_wad(value: i128, max: u128) -> Result<Wad, ContractError> {
    let value = u128::try_from(value).map_err(|_| ContractError::InvalidInput)?;
    if value > max {
        return Err(ContractError::InvalidInput);
    }
    Ok(Wad::from(value))
}

fn optional_wad(value: Option<i128>) -> Result<Option<Wad>, ContractError> {
    value
        .map(|value| {
            u128::try_from(value)
                .map(Wad::from)
                .map_err(|_| ContractError::InvalidInput)
        })
        .transpose()
}

pub(crate) fn apply_fee_change(
    env: &Env,
    performance_fee_wad: i128,
    performance_recipient: SdkAddress,
    management_fee_wad: i128,
    management_recipient: SdkAddress,
    max_growth_rate_wad: Option<i128>,
) -> Result<(), ContractError> {
    let performance_fee =
        require_non_negative_bounded_wad(performance_fee_wad, MAX_PERFORMANCE_FEE_WAD)?;
    let management_fee =
        require_non_negative_bounded_wad(management_fee_wad, MAX_MANAGEMENT_FEE_WAD)?;
    let max_rate = optional_wad(max_growth_rate_wad)?;

    let performance_kernel = kernel_address_from_sdk(env, &performance_recipient);
    let management_kernel = kernel_address_from_sdk(env, &management_recipient);
    let fees = FeesSpec::new(
        FeeSlot::new(performance_fee, performance_kernel),
        FeeSlot::new(management_fee, management_kernel),
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
    AllocString::from_utf8(value.to_bytes().to_alloc_vec()).map_err(|_| ContractError::InvalidInput)
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
    let (virtual_shares, virtual_assets) = load_virtual_offsets(env);
    config = config
        .with_fees(load_fees_spec(env)?)
        .with_virtual_offsets(virtual_shares, virtual_assets);

    let storage = SorobanStorage::new(env);
    let paused = storage.is_paused();
    let mut rbac_config = RbacConfig::with_curator(curator_kernel);
    rbac_config.add_role(governance_kernel, Role::Curator);

    load_rbac_addresses(
        env,
        &VaultDataKey::Guardians,
        Role::Sentinel,
        &mut rbac_config,
    );
    load_rbac_addresses(
        env,
        &VaultDataKey::Allocators,
        Role::Allocator,
        &mut rbac_config,
    );
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

pub(crate) type ContractVaultValueCallback<'a, T> =
    dyn for<'b> FnMut(&mut ContractVault<'b>) -> Result<T, RuntimeError> + 'a;

fn load_rbac_addresses(env: &Env, key: &soroban_sdk::Symbol, role: Role, config: &mut RbacConfig) {
    let addresses: Option<soroban_sdk::Vec<SdkAddress>> = env.storage().instance().get(key);
    if let Some(addresses) = addresses {
        for address in addresses.iter() {
            config.add_role(kernel_address_from_sdk(env, &address), role);
        }
    }
}

#[inline(never)]
fn with_contract_vault_value<T>(
    env: &Env,
    f: &mut ContractVaultValueCallback<'_, T>,
) -> Result<T, RuntimeError> {
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

#[inline(never)]
pub(crate) fn with_contract_vault(
    env: &Env,
    f: &mut ContractVaultCallback<'_>,
) -> Result<(), RuntimeError> {
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> { f(vault) };
    with_contract_vault_value(env, &mut call)
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
#[allow(deprecated)]
pub(crate) fn emit_pause_state_event(env: &Env, paused: bool) {
    let event = if paused {
        symbol_short!("paused")
    } else {
        symbol_short!("unpause")
    };
    env.events().publish((event,), ());
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
