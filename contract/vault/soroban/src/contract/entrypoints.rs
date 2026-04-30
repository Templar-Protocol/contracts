#![allow(
    clippy::too_many_arguments,
    reason = "Soroban contract entrypoints and generated client args are ABI-shaped"
)]

use super::helpers::{
    adapter_for_market, address_from_alloc_string, addresses_from_alloc_strings, apply_fee_change,
    current_supply_queue_len, emit_admin_event, emit_alloc_event, emit_pause_state_event,
    extend_storage_ttl, get_config_address, governance_caller, kernel_address_from_sdk,
    load_virtual_offsets, migrate_legacy_paused, migration_in_progress, require_contract_address,
    require_governance, require_signed, sdk_string_to_alloc, set_config_address,
    set_migration_in_progress, store_fees_spec, store_virtual_offsets,
    with_contract_vault_contract_error,
};
use super::*;
use templar_soroban_shared_types::{
    GovernanceCommand, VaultCommand, VaultCommandResult, GOVERNANCE_CONFIG_KIND_ALLOCATORS,
    GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS, GOVERNANCE_CONFIG_KIND_CURATOR,
    GOVERNANCE_CONFIG_KIND_GOVERNANCE, GOVERNANCE_CONFIG_KIND_GUARDIANS,
    GOVERNANCE_CONFIG_KIND_SENTINEL, GOVERNANCE_CONFIG_KIND_SKIM_RECIPIENT,
    GOVERNANCE_CONFIG_KIND_VIRTUAL_OFFSETS, GOVERNANCE_POLICY_KIND_CAP,
    GOVERNANCE_POLICY_KIND_FEES, GOVERNANCE_POLICY_KIND_GROUP, GOVERNANCE_POLICY_KIND_PAUSED,
    GOVERNANCE_POLICY_KIND_REMOVE_MARKET, GOVERNANCE_POLICY_KIND_RESTRICTIONS,
    GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
};
use templar_vault_kernel::state::op_state::AllocationPlanEntry;

fn required_address(
    value: Option<soroban_sdk::Address>,
) -> Result<soroban_sdk::Address, ContractError> {
    value.ok_or(ContractError::InvalidInput)
}

fn required_addresses(
    value: Option<soroban_sdk::Vec<soroban_sdk::Address>>,
) -> Result<soroban_sdk::Vec<soroban_sdk::Address>, ContractError> {
    value.ok_or(ContractError::InvalidInput)
}

fn required_i128(value: Option<i128>) -> Result<i128, ContractError> {
    value.ok_or(ContractError::InvalidInput)
}

fn apply_curator_config(env: &Env, new_curator: soroban_sdk::Address) {
    set_config_address(env, &VaultDataKey::Curator, &new_curator);
    emit_admin_event(env, symbol_short!("s_curatr"));
}

fn apply_governance_config(
    env: &Env,
    governance: soroban_sdk::Address,
) -> Result<(), ContractError> {
    require_contract_address(&governance)?;
    set_config_address(env, &VaultDataKey::Governance, &governance);
    emit_admin_event(env, symbol_short!("s_gov"));
    Ok(())
}

fn apply_sentinel_config(env: &Env, sentinel: soroban_sdk::Address) {
    env.storage()
        .instance()
        .set(&VaultDataKey::Sentinel, &sentinel);
    emit_admin_event(env, symbol_short!("s_sntnl"));
}

fn apply_guardians_config(env: &Env, guardians: soroban_sdk::Vec<soroban_sdk::Address>) {
    env.storage()
        .instance()
        .set(&VaultDataKey::Guardians, &guardians);
    emit_admin_event(env, symbol_short!("s_guards"));
}

fn apply_allocators_config(env: &Env, allocators: soroban_sdk::Vec<soroban_sdk::Address>) {
    env.storage()
        .instance()
        .set(&VaultDataKey::Allocators, &allocators);
    emit_admin_event(env, symbol_short!("s_allctr"));
}

fn apply_allowed_adapters_config(
    env: &Env,
    adapters: soroban_sdk::Vec<soroban_sdk::Address>,
) -> Result<(), ContractError> {
    let queue_len = current_supply_queue_len(env)?;
    if queue_len > 0 && adapters.len() != queue_len {
        return Err(ContractError::InvalidInput);
    }
    if adapters.is_empty() {
        env.storage()
            .instance()
            .remove(&VaultDataKey::AllowedAdapters);
    } else {
        env.storage()
            .instance()
            .set(&VaultDataKey::AllowedAdapters, &adapters);
    }
    emit_admin_event(env, symbol_short!("s_adaptr"));
    Ok(())
}

fn apply_skim_recipient_config(env: &Env, recipient: soroban_sdk::Address) {
    set_config_address(env, &VaultDataKey::SkimRecipient, &recipient);
    emit_admin_event(env, symbol_short!("s_skimr"));
}

fn apply_virtual_offsets_config(
    env: &Env,
    virtual_shares: i128,
    virtual_assets: i128,
) -> Result<(), ContractError> {
    let virtual_shares = to_u128(virtual_shares)?;
    let virtual_assets = to_u128(virtual_assets)?;
    store_virtual_offsets(env, virtual_shares, virtual_assets);
    emit_admin_event(env, symbol_short!("s_voffs"));
    Ok(())
}

fn apply_supply_queue_policy(
    env: &Env,
    caller_kernel: Address,
    target_ids: soroban_sdk::Vec<u32>,
) -> Result<(), ContractError> {
    if let Some(adapters) = env
        .storage()
        .instance()
        .get::<_, soroban_sdk::Vec<SdkAddress>>(&VaultDataKey::AllowedAdapters)
    {
        if adapters.len() != target_ids.len() {
            return Err(ContractError::InvalidInput);
        }
    }
    let mut queue_targets: Option<Vec<TargetId>> = {
        let mut v = Vec::with_capacity(target_ids.len() as usize);
        for target_id in target_ids.iter() {
            v.push(target_id);
        }
        Some(v)
    };
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        let targets = queue_targets
            .take()
            .ok_or_else(|| RuntimeError::invalid_state(""))?;
        vault.set_supply_queue(caller_kernel, targets)
    };
    with_contract_vault_contract_error(env, &mut call)
}

fn apply_cap_policy(
    env: &Env,
    caller_kernel: Address,
    market_id: u32,
    new_cap: i128,
) -> Result<(), ContractError> {
    let new_cap_u128 = to_u128(new_cap)?;
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        vault.apply_governance_cap(caller_kernel, market_id, new_cap_u128)
    };
    with_contract_vault_contract_error(env, &mut call)
}

fn apply_remove_market_policy(
    env: &Env,
    caller_kernel: Address,
    market_id: u32,
) -> Result<(), ContractError> {
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        vault.apply_governance_remove_market(caller_kernel, market_id)
    };
    with_contract_vault_contract_error(env, &mut call)
}

fn apply_restrictions_policy(
    env: &Env,
    caller_kernel: Address,
    mode: u32,
    accounts: soroban_sdk::Vec<soroban_sdk::Address>,
) -> Result<(), ContractError> {
    let mut kernel_accounts = Vec::with_capacity(accounts.len() as usize);
    for account in accounts.iter() {
        kernel_accounts.push(kernel_address_from_sdk(env, &account));
    }
    let mut restrictions = Some(match mode {
        0 => None,
        1 => Some(Restrictions::blacklist(kernel_accounts)),
        2 => Some(Restrictions::whitelist(kernel_accounts)),
        _ => return Err(ContractError::InvalidInput),
    });
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        let next_restrictions = restrictions
            .take()
            .ok_or_else(|| RuntimeError::invalid_state(""))?;
        vault.set_restrictions(caller_kernel, next_restrictions)?;
        Ok(())
    };
    with_contract_vault_contract_error(env, &mut call)?;
    emit_admin_event(env, symbol_short!("s_rstrct"));
    Ok(())
}

fn apply_group_policy(
    env: &Env,
    caller_kernel: Address,
    mode: u32,
    market_id: Option<u32>,
    cap_group_id: Option<soroban_sdk::String>,
    value: Option<i128>,
) -> Result<(), ContractError> {
    fn parse_cap_group(raw: alloc::string::String) -> Result<CapGroupId, ContractError> {
        CapGroupId::try_from(raw).map_err(|_| ContractError::InvalidInput)
    }

    let market_id = market_id.unwrap_or(0);
    let cap_group_raw = sdk_string_to_alloc(
        cap_group_id.unwrap_or_else(|| soroban_sdk::String::from_str(env, "")),
    )?;
    let internal = match mode {
        0 => CapGroupUpdate::SetCap {
            cap_group_id: parse_cap_group(cap_group_raw.clone())?,
            new_cap: match value {
                Some(raw) => Some(to_u128(raw)?),
                None => None,
            },
        },
        1 => CapGroupUpdate::SetRelativeCap {
            cap_group_id: parse_cap_group(cap_group_raw.clone())?,
            new_relative_cap: match value {
                Some(raw) => Some(Wad::from(to_u128(raw)?)),
                None => None,
            },
        },
        2 => {
            let group = if cap_group_raw.is_empty() {
                None
            } else {
                Some(parse_cap_group(cap_group_raw)?)
            };
            CapGroupUpdate::SetMembership {
                market_id,
                cap_group_id: group,
            }
        }
        _ => return Err(ContractError::InvalidInput),
    };
    let mut internal = Some(internal);
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        let update = internal
            .take()
            .ok_or_else(|| RuntimeError::invalid_state(""))?;
        vault.apply_governance_cap_group_update(caller_kernel, update)
    };
    with_contract_vault_contract_error(env, &mut call)
}

fn apply_paused_policy(
    env: &Env,
    caller_kernel: Address,
    paused: bool,
) -> Result<(), ContractError> {
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        vault.pause(caller_kernel, paused)
    };
    with_contract_vault_contract_error(env, &mut call)?;
    emit_pause_state_event(env, paused);
    runtime_to_contract(crate::effects::publish_kernel_event(
        env,
        &templar_vault_kernel::effects::KernelEvent::PauseUpdated { paused },
    ))?;
    Ok(())
}

fn apply_fees_policy(
    env: &Env,
    accounts: soroban_sdk::Vec<soroban_sdk::Address>,
    performance_fee_wad: i128,
    management_fee_wad: i128,
    max_growth_rate_wad: Option<i128>,
) -> Result<(), ContractError> {
    if accounts.len() != 2 {
        return Err(ContractError::InvalidInput);
    }
    let performance_recipient = accounts.get_unchecked(0);
    let management_recipient = accounts.get_unchecked(1);
    apply_fee_change(
        env,
        performance_fee_wad,
        performance_recipient,
        management_fee_wad,
        management_recipient,
        max_growth_rate_wad,
    )?;
    emit_admin_event(env, symbol_short!("s_fees"));
    Ok(())
}

fn deposit_with_min_impl(
    env: &Env,
    owner: soroban_sdk::Address,
    receiver: soroban_sdk::Address,
    assets: i128,
    min_shares_out: i128,
) -> Result<i128, ContractError> {
    require_signed(&owner);
    if assets <= 0 {
        return Err(ContractError::InvalidInput);
    }

    let assets_u128 = to_u128(assets)?;
    let min_shares_u128 = if min_shares_out < 0 {
        return Err(ContractError::InvalidInput);
    } else {
        to_u128(min_shares_out)?
    };
    let now_ns = ledger_timestamp_ns(env)?;

    let mut shares_minted = 0u128;
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        let (caller_k, receiver_k) = vault.map_pair(env, &owner, &receiver)?;
        let result = vault.deposit(caller_k, receiver_k, assets_u128, min_shares_u128, now_ns)?;
        shares_minted = result.shares_minted;
        Ok(())
    };
    with_contract_vault_contract_error(env, &mut call)?;
    to_i128(shares_minted)
}

fn request_withdraw_impl(
    env: &Env,
    owner: soroban_sdk::Address,
    receiver: soroban_sdk::Address,
    shares: i128,
    min_assets_out: i128,
) -> Result<u64, ContractError> {
    require_signed(&owner);
    if shares <= 0 {
        return Err(ContractError::InvalidInput);
    }
    let shares_u128 = to_u128(shares)?;
    let min_assets_u128 = if min_assets_out < 0 {
        return Err(ContractError::InvalidInput);
    } else {
        to_u128(min_assets_out)?
    };
    let now_ns = ledger_timestamp_ns(env)?;

    let mut request_id = 0u64;
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        let (caller_k, receiver_k) = vault.map_pair(env, &owner, &receiver)?;
        let result =
            vault.request_withdraw(caller_k, receiver_k, shares_u128, min_assets_u128, now_ns)?;
        request_id = result.request_id;
        Ok(())
    };
    with_contract_vault_contract_error(env, &mut call)?;
    Ok(request_id)
}

fn execute_withdraw_impl(env: &Env, caller: soroban_sdk::Address) -> Result<(), ContractError> {
    require_signed(&caller);
    let now_ns = ledger_timestamp_ns(env)?;

    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        let caller_k = vault.map_caller(env, &caller)?;
        vault.execute_withdraw(caller_k, now_ns).map(|_| ())
    };
    with_contract_vault_contract_error(env, &mut call)
}

fn abort_withdrawing_impl(
    env: &Env,
    caller: soroban_sdk::Address,
    op_id: u64,
) -> Result<(), ContractError> {
    require_signed(&caller);
    let now_ns = ledger_timestamp_ns(env)?;

    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        let caller_k = vault.map_caller(env, &caller)?;
        vault.abort_withdrawing(caller_k, op_id, now_ns).map(|_| ())
    };
    with_contract_vault_contract_error(env, &mut call)
}

fn allocate_impl(
    env: &Env,
    caller: soroban_sdk::Address,
    market: u32,
    amount: i128,
    supply: bool,
) -> Result<i128, ContractError> {
    require_signed(&caller);
    let caller_kernel = kernel_address_from_sdk(env, &caller);
    let mut preauth = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        vault.authorize(ActionKind::BeginAllocating, caller_kernel)
    };
    with_contract_vault_contract_error(env, &mut preauth)?;
    let adapter = adapter_for_market(env, market)?;
    if amount <= 0 {
        return Err(ContractError::InvalidInput);
    }
    let now_ns = ledger_timestamp_ns(env)?;
    let asset_token = get_config_address(env, &VaultDataKey::AssetToken)?;
    let mut new_external: u128 = 0;
    let emitted_amount = if supply {
        let amount_u128 = to_u128(amount)?;
        soroban_sdk::token::Client::new(env, &asset_token).transfer(
            &env.current_contract_address(),
            &adapter,
            &amount,
        );
        invoke_supply(env, &adapter, &asset_token, amount);
        let observed_total_assets = to_u128(invoke_total_assets(env, &adapter, &asset_token))?;

        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let plan = vec![AllocationPlanEntry::new(market, amount_u128)];
            let op_id = vault.begin_allocation_internal(caller_kernel, &plan, now_ns)?;
            new_external = vault.complete_supply_allocation(
                caller_kernel,
                market,
                observed_total_assets,
                op_id,
                now_ns,
            )?;
            Ok(())
        };
        with_contract_vault_contract_error(env, &mut call)?;
        amount
    } else {
        let realized_amount = invoke_progress_withdrawal(env, &adapter, &asset_token, amount);
        let realized_amount_u128 = to_u128(realized_amount)?;

        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let op_id = vault.begin_allocation_withdraw_internal(caller_kernel, market, now_ns)?;
            new_external = vault.complete_withdraw_allocation(
                caller_kernel,
                market,
                realized_amount_u128,
                op_id,
                now_ns,
            )?;
            Ok(())
        };
        with_contract_vault_contract_error(env, &mut call)?;
        realized_amount
    };
    emit_alloc_event(env, market, emitted_amount, supply);
    to_i128(new_external)
}

fn refresh_markets_impl(
    env: &Env,
    caller: soroban_sdk::Address,
    markets: soroban_sdk::Vec<u32>,
) -> Result<i128, ContractError> {
    require_signed(&caller);
    let caller_kernel = kernel_address_from_sdk(env, &caller);
    let mut preauth = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        vault.authorize(ActionKind::BeginRefreshing, caller_kernel)
    };
    with_contract_vault_contract_error(env, &mut preauth)?;
    let now_ns = ledger_timestamp_ns(env)?;
    let asset_token = get_config_address(env, &VaultDataKey::AssetToken)?;
    let mut refreshed_positions = Vec::with_capacity(markets.len() as usize);
    for market in markets.iter() {
        let adapter = adapter_for_market(env, market)?;
        let total_assets = invoke_total_assets(env, &adapter, &asset_token);
        refreshed_positions.push((market, to_u128(total_assets)?));
    }

    let mut markets_vec: Option<Vec<TargetId>> = Some(
        refreshed_positions
            .iter()
            .map(|(market, _)| *market)
            .collect(),
    );

    let mut new_external: u128 = 0;
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        let markets = markets_vec
            .take()
            .ok_or_else(|| RuntimeError::invalid_state(""))?;
        let op_id = vault.begin_refreshing(caller_kernel, markets, now_ns)?;
        let result = vault.complete_refresh_with_positions(
            caller_kernel,
            &refreshed_positions,
            op_id,
            now_ns,
        )?;
        new_external = result.new_external_assets;
        Ok(())
    };
    with_contract_vault_contract_error(env, &mut call)?;
    to_i128(new_external)
}

fn set_governance_config_impl(
    env: &Env,
    caller: soroban_sdk::Address,
    kind: u32,
    primary: Option<soroban_sdk::Address>,
    many: Option<soroban_sdk::Vec<soroban_sdk::Address>>,
    value_a: Option<i128>,
    value_b: Option<i128>,
) -> Result<(), ContractError> {
    require_governance(env, &caller)?;
    match kind {
        GOVERNANCE_CONFIG_KIND_CURATOR => apply_curator_config(env, required_address(primary)?),
        GOVERNANCE_CONFIG_KIND_GOVERNANCE => {
            apply_governance_config(env, required_address(primary)?)?
        }
        GOVERNANCE_CONFIG_KIND_SENTINEL => apply_sentinel_config(env, required_address(primary)?),
        GOVERNANCE_CONFIG_KIND_GUARDIANS => apply_guardians_config(env, required_addresses(many)?),
        GOVERNANCE_CONFIG_KIND_ALLOCATORS => {
            apply_allocators_config(env, required_addresses(many)?)
        }
        GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS => {
            apply_allowed_adapters_config(env, required_addresses(many)?)?
        }
        GOVERNANCE_CONFIG_KIND_SKIM_RECIPIENT => {
            apply_skim_recipient_config(env, required_address(primary)?)
        }
        GOVERNANCE_CONFIG_KIND_VIRTUAL_OFFSETS => {
            apply_virtual_offsets_config(env, required_i128(value_a)?, required_i128(value_b)?)?
        }
        _ => return Err(ContractError::InvalidInput),
    }
    Ok(())
}

fn set_governance_policy_impl(
    env: &Env,
    caller: soroban_sdk::Address,
    kind: u32,
    target_ids: Option<soroban_sdk::Vec<u32>>,
    mode: Option<u32>,
    accounts: Option<soroban_sdk::Vec<soroban_sdk::Address>>,
    market_id: Option<u32>,
    cap_group_id: Option<soroban_sdk::String>,
    value: Option<i128>,
    value_b: Option<i128>,
    value_c: Option<i128>,
) -> Result<(), ContractError> {
    let caller_kernel = governance_caller(env, &caller)?;
    match kind {
        GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE => apply_supply_queue_policy(
            env,
            caller_kernel,
            target_ids.ok_or(ContractError::InvalidInput)?,
        ),
        GOVERNANCE_POLICY_KIND_CAP => apply_cap_policy(
            env,
            caller_kernel,
            market_id.ok_or(ContractError::InvalidInput)?,
            required_i128(value)?,
        ),
        GOVERNANCE_POLICY_KIND_REMOVE_MARKET => apply_remove_market_policy(
            env,
            caller_kernel,
            market_id.ok_or(ContractError::InvalidInput)?,
        ),
        GOVERNANCE_POLICY_KIND_RESTRICTIONS => apply_restrictions_policy(
            env,
            caller_kernel,
            mode.ok_or(ContractError::InvalidInput)?,
            accounts.ok_or(ContractError::InvalidInput)?,
        ),
        GOVERNANCE_POLICY_KIND_GROUP => apply_group_policy(
            env,
            caller_kernel,
            mode.ok_or(ContractError::InvalidInput)?,
            market_id,
            cap_group_id,
            value,
        ),
        GOVERNANCE_POLICY_KIND_PAUSED => apply_paused_policy(
            env,
            caller_kernel,
            mode.ok_or(ContractError::InvalidInput)? != 0,
        ),
        GOVERNANCE_POLICY_KIND_FEES => apply_fees_policy(
            env,
            accounts.ok_or(ContractError::InvalidInput)?,
            required_i128(value)?,
            required_i128(value_b)?,
            value_c,
        ),
        _ => Err(ContractError::InvalidInput),
    }
}

fn skim_impl(
    env: &Env,
    caller: soroban_sdk::Address,
    token: soroban_sdk::Address,
) -> Result<(), ContractError> {
    require_governance(env, &caller)?;
    let asset = get_config_address(env, &VaultDataKey::AssetToken)?;
    let share = get_config_address(env, &VaultDataKey::ShareToken)?;
    if token == asset || token == share {
        return Err(ContractError::InvalidInput);
    }

    let recipient = get_config_address(env, &VaultDataKey::SkimRecipient)?;
    let client = soroban_sdk::token::Client::new(env, &token);
    let balance = client.balance(&env.current_contract_address());
    if balance <= 0 {
        return Err(ContractError::InvalidState);
    }

    client.transfer(&env.current_contract_address(), &recipient, &balance);
    emit_admin_event(env, symbol_short!("skim"));
    Ok(())
}

fn resync_idle_balance_impl(env: &Env) -> Result<(), ContractError> {
    let now_ns = ledger_timestamp_ns(env)?;
    let cooldown_ns = 120_000_000_000u64;
    let last_key = VaultDataKey::IdleResyncLastNs;
    let last_ns = env.storage().instance().get(&last_key).unwrap_or(0u64);
    if last_ns != 0 && now_ns.saturating_sub(last_ns) < cooldown_ns {
        return Err(ContractError::InvalidState);
    }
    let asset_token = get_config_address(env, &VaultDataKey::AssetToken)?;
    let client = soroban_sdk::token::Client::new(env, &asset_token);
    let actual_balance = to_u128(client.balance(&env.current_contract_address()))?;
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        let state = vault.state_mut()?;
        let before_idle = state.idle_assets;
        if !state.op_state.is_idle() {
            return Err(RuntimeError::invalid_state(""));
        }

        state.idle_assets = actual_balance;
        state.sync_total_assets();

        if actual_balance > before_idle {
            let delta = actual_balance - before_idle;
            state.fee_anchor.total_assets = state.fee_anchor.total_assets.saturating_add(delta);
        }

        vault.save_state()?;
        Ok(())
    };
    with_contract_vault_contract_error(env, &mut call)?;
    env.storage().instance().set(&last_key, &now_ns);
    emit_admin_event(env, symbol_short!("resync"));
    Ok(())
}

fn cancel_migration_impl(env: &Env, caller: soroban_sdk::Address) -> Result<(), ContractError> {
    require_governance(env, &caller)?;
    if !migration_in_progress(env) {
        return Err(ContractError::InvalidState);
    }

    set_migration_in_progress(env, false);
    emit_admin_event(env, symbol_short!("cnc_migr"));
    Ok(())
}

fn execute_public_command(
    env: &Env,
    command: VaultCommand,
) -> Result<VaultCommandResult, ContractError> {
    match command {
        VaultCommand::DepositWithMin {
            owner,
            receiver,
            assets,
            min_shares_out,
        } => Ok(VaultCommandResult::I128(deposit_with_min_impl(
            env,
            address_from_alloc_string(env, &owner)?,
            address_from_alloc_string(env, &receiver)?,
            assets,
            min_shares_out,
        )?)),
        VaultCommand::RequestWithdraw {
            owner,
            receiver,
            shares,
            min_assets_out,
        } => Ok(VaultCommandResult::U64(request_withdraw_impl(
            env,
            address_from_alloc_string(env, &owner)?,
            address_from_alloc_string(env, &receiver)?,
            shares,
            min_assets_out,
        )?)),
        VaultCommand::ExecuteWithdraw { caller } => {
            execute_withdraw_impl(env, address_from_alloc_string(env, &caller)?)?;
            Ok(VaultCommandResult::Unit)
        }
        VaultCommand::AbortWithdrawing { caller, op_id } => {
            abort_withdrawing_impl(env, address_from_alloc_string(env, &caller)?, op_id)?;
            Ok(VaultCommandResult::Unit)
        }
        VaultCommand::Allocate {
            caller,
            market,
            amount,
            supply,
        } => Ok(VaultCommandResult::I128(allocate_impl(
            env,
            address_from_alloc_string(env, &caller)?,
            market,
            amount,
            supply,
        )?)),
        VaultCommand::RefreshMarkets { caller, markets } => {
            let mut sdk_markets = soroban_sdk::Vec::new(env);
            for market in markets {
                sdk_markets.push_back(market);
            }
            Ok(VaultCommandResult::I128(refresh_markets_impl(
                env,
                address_from_alloc_string(env, &caller)?,
                sdk_markets,
            )?))
        }
        VaultCommand::ResyncIdleBalance => {
            resync_idle_balance_impl(env)?;
            Ok(VaultCommandResult::Unit)
        }
        VaultCommand::CancelMigration { caller } => {
            cancel_migration_impl(env, address_from_alloc_string(env, &caller)?)?;
            Ok(VaultCommandResult::Unit)
        }
        VaultCommand::ExtendTtl => {
            extend_storage_ttl(env);
            Ok(VaultCommandResult::Unit)
        }
    }
}

fn execute_governance_command(
    env: &Env,
    caller: soroban_sdk::Address,
    command: GovernanceCommand,
) -> Result<(), ContractError> {
    match command {
        GovernanceCommand::SetGovernanceConfig {
            kind,
            primary,
            many,
            value_a,
            value_b,
        } => {
            let primary = primary
                .as_ref()
                .map(|value| address_from_alloc_string(env, value))
                .transpose()?;
            let many = many
                .as_ref()
                .map(|values| addresses_from_alloc_strings(env, values))
                .transpose()?;
            set_governance_config_impl(env, caller, kind, primary, many, value_a, value_b)
        }
        GovernanceCommand::SetGovernancePolicy {
            kind,
            target_ids,
            mode,
            accounts,
            market_id,
            cap_group_id,
            value,
            value_b,
            value_c,
        } => {
            let accounts = accounts
                .as_ref()
                .map(|values| addresses_from_alloc_strings(env, values))
                .transpose()?;
            let cap_group_id = cap_group_id
                .as_ref()
                .map(|value| soroban_sdk::String::from_str(env, value));
            let target_ids = target_ids.map(|ids| {
                let mut result = soroban_sdk::Vec::new(env);
                for id in ids {
                    result.push_back(id);
                }
                result
            });
            set_governance_policy_impl(
                env,
                caller,
                kind,
                target_ids,
                mode,
                accounts,
                market_id,
                cap_group_id,
                value,
                value_b,
                value_c,
            )
        }
        GovernanceCommand::Skim { token } => {
            skim_impl(env, caller, address_from_alloc_string(env, &token)?)
        }
    }
}
#[contract]
pub struct SorobanVaultContract;

#[contractimpl]
impl SorobanVaultContract {
    pub fn initialize(
        env: Env,
        curator: soroban_sdk::Address,
        governance: soroban_sdk::Address,
        asset_token: soroban_sdk::Address,
        share_token: soroban_sdk::Address,
        virtual_shares: i128,
        virtual_assets: i128,
    ) -> Result<(), ContractError> {
        if env.storage().instance().has(&VaultDataKey::Initialized) {
            return Err(ContractError::AlreadyInitialized);
        }

        let virtual_shares = to_u128(virtual_shares)?;
        let virtual_assets = to_u128(virtual_assets)?;

        set_config_address(&env, &VaultDataKey::Curator, &curator);
        set_config_address(&env, &VaultDataKey::Governance, &governance);
        set_config_address(&env, &VaultDataKey::AssetToken, &asset_token);
        set_config_address(&env, &VaultDataKey::ShareToken, &share_token);
        set_config_address(&env, &VaultDataKey::SkimRecipient, &governance);
        store_virtual_offsets(&env, virtual_shares, virtual_assets);
        env.storage()
            .instance()
            .set(&VaultDataKey::Initialized, &true);
        runtime_to_contract(store_fees_spec(&env, &FeesSpec::zero()))?;

        let mut storage = SorobanStorage::new(&env);
        runtime_to_contract(storage.save_state(&VaultState::default()))?;
        runtime_to_contract(storage.save_paused(false))?;
        Ok(())
    }

    pub fn execute(env: Env, payload: Bytes) -> Result<Bytes, ContractError> {
        let command = decode_command(&payload)?;
        let result = execute_public_command(&env, command)?;
        encode_command_result(&env, &result)
    }

    pub fn execute_governance(
        env: Env,
        caller: soroban_sdk::Address,
        payload: Bytes,
    ) -> Result<(), ContractError> {
        let command = GovernanceCommand::decode(&payload.to_alloc_vec())
            .map_err(|_| ContractError::InvalidInput)?;
        execute_governance_command(&env, caller, command)
    }

    #[allow(
        clippy::type_complexity,
        reason = "proxy view is a compact ABI surface consumed by tests and tooling"
    )]
    #[allow(clippy::too_many_lines)]
    pub fn proxy_view(
        env: Env,
        owner: soroban_sdk::Address,
        assets: i128,
        shares: i128,
    ) -> Result<
        (
            (
                (
                    soroban_sdk::Address,
                    soroban_sdk::Address,
                    soroban_sdk::Address,
                    soroban_sdk::Address,
                ),
                (i128, i128, bool),
                (i128, i128, i128, i128),
                (i128, u64, i128, i128),
            ),
            (
                soroban_sdk::Vec<u32>,
                soroban_sdk::Vec<(soroban_sdk::String, i128, i128)>,
            ),
            (i128, i128, i128, i128, i128, i128, i128, i128),
        ),
        ContractError,
    > {
        let (virtual_shares, virtual_assets) = load_virtual_offsets(&env);
        let storage = SorobanStorage::new(&env);
        let mut queue = soroban_sdk::Vec::new(&env);
        let mut groups = soroban_sdk::Vec::new(&env);
        let (state, config) = load_state_and_config(&env)?;
        let total_shares = to_i128(state.total_shares)?;
        let idle_assets = to_i128(state.idle_assets)?;
        let external_assets = to_i128(state.external_assets)?;
        let total_assets = to_i128(state.total_assets)?;
        let fee_info = (
            state.fee_anchor.total_assets as i128,
            state.fee_anchor.timestamp_ns.as_u64(),
            u128::from(config.fees.management.fee_wad) as i128,
            u128::from(config.fees.performance.fee_wad) as i128,
        );
        let policy_state = runtime_to_contract(storage.load_policy_state())?.unwrap_or_default();
        for entry in policy_state.supply_queue().entries() {
            queue.push_back(entry.target_id);
        }
        for (id, rec) in policy_state.cap_groups().iter() {
            let sdk_id = soroban_sdk::String::from_str(&env, id.as_str());
            let abs_cap = rec.cap.absolute_cap().map(|c| c as i128).unwrap_or(0);
            groups.push_back((sdk_id, abs_cap, rec.principal as i128));
        }
        let convert_to_shares_value = if assets <= 0 {
            0
        } else {
            let assets_u128 = to_u128(assets)?;
            to_i128(convert_to_shares(&state, &config, assets_u128))?
        };

        let convert_to_assets_value = if shares <= 0 {
            0
        } else {
            let shares_u128 = to_u128(shares)?;
            to_i128(convert_to_assets(&state, &config, shares_u128))?
        };

        let (max_deposit_value, max_mint_value) = if state.op_state.is_idle() && !config.paused {
            let max_assets = u128::MAX
                .saturating_sub(state.total_assets)
                .min(i128::MAX as u128) as i128;
            let max_shares = u128::MAX
                .saturating_sub(state.total_shares)
                .min(i128::MAX as u128) as i128;
            (max_assets, max_shares)
        } else {
            (0, 0)
        };

        let owner_shares = share_balance(&env, &owner).max(0) as u128;
        let (max_withdraw_value, max_redeem_value) = if state.op_state.is_idle() {
            let max_redeem_u128 =
                owner_shares.min(convert_to_shares(&state, &config, state.idle_assets));
            let max_withdraw_u128 =
                convert_to_assets(&state, &config, owner_shares).min(state.idle_assets);
            (to_i128(max_withdraw_u128)?, to_i128(max_redeem_u128)?)
        } else {
            (0, 0)
        };

        let preview_mint_value = if shares <= 0 {
            0
        } else {
            let shares_u128 = to_u128(shares)?;
            to_i128(convert_to_assets_ceil(&state, &config, shares_u128))?
        };

        let preview_withdraw_value = if assets <= 0 {
            0
        } else {
            let assets_u128 = to_u128(assets)?;
            to_i128(convert_to_shares_ceil(&state, &config, assets_u128))?
        };

        Ok((
            (
                (
                    get_config_address(&env, &VaultDataKey::Curator)?,
                    get_config_address(&env, &VaultDataKey::Governance)?,
                    get_config_address(&env, &VaultDataKey::AssetToken)?,
                    get_config_address(&env, &VaultDataKey::ShareToken)?,
                ),
                (
                    to_i128(virtual_shares)?,
                    to_i128(virtual_assets)?,
                    storage.is_paused(),
                ),
                (total_shares, idle_assets, external_assets, total_assets),
                fee_info,
            ),
            (queue, groups),
            (
                convert_to_shares_value,
                convert_to_assets_value,
                max_deposit_value,
                max_mint_value,
                max_withdraw_value,
                max_redeem_value,
                preview_mint_value,
                preview_withdraw_value,
            ),
        ))
    }

    pub fn upgrade(
        env: Env,
        new_wasm_hash: BytesN<32>,
        operator: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        require_governance(&env, &operator)?;
        set_migration_in_progress(&env, true);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        emit_admin_event(&env, symbol_short!("upgrade"));
        Ok(())
    }

    pub fn migrate(env: Env, operator: soroban_sdk::Address) -> Result<(), ContractError> {
        require_governance(&env, &operator)?;
        if !migration_in_progress(&env) {
            return Err(ContractError::InvalidState);
        }

        migrate_legacy_paused(&env);
        extend_storage_ttl(&env);
        set_migration_in_progress(&env, false);
        emit_admin_event(&env, symbol_short!("migrate"));
        Ok(())
    }
}
