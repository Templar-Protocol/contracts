use super::helpers::{
    adapter_for_market, apply_fee_change, current_supply_queue_len, emit_admin_event,
    emit_alloc_event, emit_pause_state_event, extend_storage_ttl, get_config_address,
    governance_caller, kernel_address_from_sdk, load_virtual_offsets, max_deposit_or_mint,
    max_withdraw_or_redeem, migrate_legacy_paused, migration_in_progress, query_vault_field,
    query_vault_snapshot, require_contract_address, require_governance, require_signed,
    sdk_string_to_alloc, set_config_address, set_migration_in_progress, store_fees_spec,
    store_virtual_offsets, with_contract_vault_contract_error,
};
use super::*;

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
    ) -> Result<(), ContractError> {
        Self::initialize_with_virtual_offsets(
            env,
            curator,
            governance,
            asset_token,
            share_token,
            0,
            0,
        )
    }

    pub fn initialize_with_virtual_offsets(
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
        let versioned = VersionedState::new(VaultState::default());
        runtime_to_contract(storage.save_state(&versioned))?;
        runtime_to_contract(storage.save_paused(false))?;
        Ok(())
    }

    pub fn set_virtual_offsets(
        env: Env,
        caller: soroban_sdk::Address,
        virtual_shares: i128,
        virtual_assets: i128,
    ) -> Result<(), ContractError> {
        require_governance(&env, &caller)?;
        let virtual_shares = to_u128(virtual_shares)?;
        let virtual_assets = to_u128(virtual_assets)?;
        store_virtual_offsets(&env, virtual_shares, virtual_assets);
        emit_admin_event(&env, symbol_short!("s_voffs"));
        Ok(())
    }

    pub fn deposit_with_min(
        env: Env,
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
        let now_ns = ledger_timestamp_ns(&env)?;

        let mut shares_minted = 0u128;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let (caller_k, receiver_k) = vault.map_pair(&env, &owner, &receiver)?;
            let result =
                vault.deposit(caller_k, receiver_k, assets_u128, min_shares_u128, now_ns)?;
            shares_minted = result.shares_minted;
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        to_i128(shares_minted)
    }

    pub fn request_withdraw(
        env: Env,
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
        let now_ns = ledger_timestamp_ns(&env)?;

        let mut request_id = 0u64;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let (caller_k, receiver_k) = vault.map_pair(&env, &owner, &receiver)?;
            let result = vault.request_withdraw(
                caller_k,
                receiver_k,
                shares_u128,
                min_assets_u128,
                now_ns,
            )?;
            request_id = result.request_id;
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        Ok(request_id)
    }

    pub fn execute_withdraw(env: Env, caller: soroban_sdk::Address) -> Result<(), ContractError> {
        require_signed(&caller);
        let now_ns = ledger_timestamp_ns(&env)?;

        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let caller_k = vault.map_caller(&env, &caller)?;
            vault.execute_withdraw(caller_k, now_ns).map(|_| ())
        };
        with_contract_vault_contract_error(&env, &mut call)
    }

    pub fn allocate_supply(
        env: Env,
        caller: soroban_sdk::Address,
        market: u32,
        amount: i128,
    ) -> Result<i128, ContractError> {
        require_signed(&caller);
        let caller_kernel = kernel_address_from_sdk(&env, &caller);
        let mut preauth = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.authorize(ActionKind::BeginAllocating, caller_kernel)
        };
        with_contract_vault_contract_error(&env, &mut preauth)?;
        let adapter = adapter_for_market(&env, market)?;
        if amount <= 0 {
            return Err(ContractError::InvalidInput);
        }
        let amount_u128 = to_u128(amount)?;
        let now_ns = ledger_timestamp_ns(&env)?;
        let asset_token = get_config_address(&env, &VaultDataKey::AssetToken)?;
        soroban_sdk::token::Client::new(&env, &asset_token).transfer(
            &env.current_contract_address(),
            &adapter,
            &amount,
        );
        invoke_supply(&env, &adapter, &asset_token, amount);
        let observed_total_assets = to_u128(invoke_total_assets(&env, &adapter, &asset_token))?;

        let mut new_external: u128 = 0;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let plan = vec![(market.into(), amount_u128)];
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
        with_contract_vault_contract_error(&env, &mut call)?;

        emit_alloc_event(&env, market, amount, true);
        to_i128(new_external)
    }

    pub fn allocate_withdraw(
        env: Env,
        caller: soroban_sdk::Address,
        market: u32,
        amount: i128,
    ) -> Result<i128, ContractError> {
        require_signed(&caller);
        let caller_kernel = kernel_address_from_sdk(&env, &caller);
        let mut preauth = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.authorize(ActionKind::BeginAllocating, caller_kernel)
        };
        with_contract_vault_contract_error(&env, &mut preauth)?;
        let adapter = adapter_for_market(&env, market)?;
        if amount <= 0 {
            return Err(ContractError::InvalidInput);
        }

        let asset_token = get_config_address(&env, &VaultDataKey::AssetToken)?;
        let realized_amount = invoke_progress_withdrawal(&env, &adapter, &asset_token, amount);
        let realized_amount_u128 = to_u128(realized_amount)?;
        let now_ns = ledger_timestamp_ns(&env)?;

        let mut new_external: u128 = 0;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let op_id =
                vault.begin_allocation_withdraw_internal(caller_kernel, market.into(), now_ns)?;
            new_external = vault.complete_withdraw_allocation(
                caller_kernel,
                market,
                realized_amount_u128,
                op_id,
                now_ns,
            )?;
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        emit_alloc_event(&env, market, realized_amount, false);
        to_i128(new_external)
    }

    pub fn refresh_markets(
        env: Env,
        caller: soroban_sdk::Address,
        markets: soroban_sdk::Vec<u32>,
    ) -> Result<i128, ContractError> {
        require_signed(&caller);
        let caller_kernel = kernel_address_from_sdk(&env, &caller);
        let mut preauth = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.authorize(ActionKind::BeginRefreshing, caller_kernel)
        };
        with_contract_vault_contract_error(&env, &mut preauth)?;
        let now_ns = ledger_timestamp_ns(&env)?;
        let asset_token = get_config_address(&env, &VaultDataKey::AssetToken)?;
        let mut refreshed_positions = Vec::with_capacity(markets.len() as usize);
        for market in markets.iter() {
            let adapter = adapter_for_market(&env, market)?;
            let total_assets = invoke_total_assets(&env, &adapter, &asset_token);
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
                .ok_or_else(|| RuntimeError::invalid_state("refresh plan already consumed"))?;
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
        with_contract_vault_contract_error(&env, &mut call)?;
        to_i128(new_external)
    }

    pub fn set_paused(
        env: Env,
        caller: soroban_sdk::Address,
        paused: bool,
    ) -> Result<(), ContractError> {
        let caller_kernel = governance_caller(&env, &caller)?;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.pause(caller_kernel, paused)
        };
        with_contract_vault_contract_error(&env, &mut call)?;

        emit_pause_state_event(&env, paused);
        runtime_to_contract(crate::effects::publish_kernel_event(
            &env,
            &templar_vault_kernel::effects::KernelEvent::PauseUpdated { paused },
        ))?;
        Ok(())
    }

    pub fn set_curator(
        env: Env,
        caller: soroban_sdk::Address,
        new_curator: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        require_governance(&env, &caller)?;
        set_config_address(&env, &VaultDataKey::Curator, &new_curator);
        emit_admin_event(&env, symbol_short!("s_curatr"));
        Ok(())
    }

    pub fn set_governance(
        env: Env,
        caller: soroban_sdk::Address,
        governance: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        require_governance(&env, &caller)?;
        require_contract_address(&governance)?;
        set_config_address(&env, &VaultDataKey::Governance, &governance);
        emit_admin_event(&env, symbol_short!("s_gov"));
        Ok(())
    }

    pub fn set_supply_queue(
        env: Env,
        caller: soroban_sdk::Address,
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
        let caller_kernel = governance_caller(&env, &caller)?;
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
                .ok_or_else(|| RuntimeError::invalid_state("supply queue already consumed"))?;
            vault.set_supply_queue(caller_kernel, targets)
        };
        with_contract_vault_contract_error(&env, &mut call)
    }

    pub fn set_cap(
        env: Env,
        caller: soroban_sdk::Address,
        market_id: u32,
        new_cap: i128,
    ) -> Result<(), ContractError> {
        let caller_kernel = governance_caller(&env, &caller)?;
        let new_cap_u128 = to_u128(new_cap)?;

        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.set_cap(caller_kernel, market_id, new_cap_u128)
        };
        with_contract_vault_contract_error(&env, &mut call)
    }

    pub fn remove_market(
        env: Env,
        caller: soroban_sdk::Address,
        market_id: u32,
    ) -> Result<(), ContractError> {
        let caller_kernel = governance_caller(&env, &caller)?;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.remove_market(caller_kernel, market_id)
        };
        with_contract_vault_contract_error(&env, &mut call)
    }

    pub fn set_group_cap(
        env: Env,
        caller: soroban_sdk::Address,
        cap_group_id: soroban_sdk::String,
        new_cap: i128,
    ) -> Result<(), ContractError> {
        let caller_kernel = governance_caller(&env, &caller)?;
        let internal = CapGroupUpdate::SetCap {
            cap_group_id: sdk_string_to_alloc(cap_group_id)?.into(),
            new_cap: to_u128(new_cap)?,
        };
        let mut internal = Some(internal);
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let update = internal
                .take()
                .ok_or_else(|| RuntimeError::invalid_state("cap group update already consumed"))?;
            vault.update_cap_group(caller_kernel, update)
        };
        with_contract_vault_contract_error(&env, &mut call)
    }

    pub fn set_group_rel_cap(
        env: Env,
        caller: soroban_sdk::Address,
        cap_group_id: soroban_sdk::String,
        new_relative_cap_wad: i128,
    ) -> Result<(), ContractError> {
        let caller_kernel = governance_caller(&env, &caller)?;
        let internal = CapGroupUpdate::SetRelativeCap {
            cap_group_id: sdk_string_to_alloc(cap_group_id)?.into(),
            new_relative_cap_wad: to_u128(new_relative_cap_wad)?,
        };
        let mut internal = Some(internal);
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let update = internal
                .take()
                .ok_or_else(|| RuntimeError::invalid_state("cap group update already consumed"))?;
            vault.update_cap_group(caller_kernel, update)
        };
        with_contract_vault_contract_error(&env, &mut call)
    }

    pub fn set_group_member(
        env: Env,
        caller: soroban_sdk::Address,
        market_id: u32,
        cap_group_id: soroban_sdk::String,
    ) -> Result<(), ContractError> {
        let caller_kernel = governance_caller(&env, &caller)?;
        let s = sdk_string_to_alloc(cap_group_id)?;
        let group = if s.is_empty() { None } else { Some(s.into()) };
        let internal = CapGroupUpdate::SetMembership {
            market_id,
            cap_group_id: group,
        };
        let mut internal = Some(internal);
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let update = internal
                .take()
                .ok_or_else(|| RuntimeError::invalid_state("cap group update already consumed"))?;
            vault.update_cap_group(caller_kernel, update)
        };
        with_contract_vault_contract_error(&env, &mut call)
    }

    pub fn set_fees(
        env: Env,
        caller: soroban_sdk::Address,
        performance_fee_wad: i128,
        performance_recipient: soroban_sdk::Address,
        management_fee_wad: i128,
        management_recipient: soroban_sdk::Address,
        max_growth_rate_wad: Option<i128>,
    ) -> Result<(), ContractError> {
        require_governance(&env, &caller)?;
        apply_fee_change(
            &env,
            performance_fee_wad,
            performance_recipient,
            management_fee_wad,
            management_recipient,
            max_growth_rate_wad,
        )?;
        emit_admin_event(&env, symbol_short!("s_fees"));
        Ok(())
    }

    pub fn accept_fees(env: Env, caller: soroban_sdk::Address) -> Result<(), ContractError> {
        require_governance(&env, &caller)?;
        Err(ContractError::InvalidState)
    }

    pub fn revoke_pending_fees(
        env: Env,
        caller: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        require_governance(&env, &caller)?;
        Err(ContractError::InvalidState)
    }

    pub fn pending_fees_valid_at(_env: Env) -> Result<Option<u64>, ContractError> {
        Ok(None)
    }

    pub fn set_restrictions(
        env: Env,
        caller: soroban_sdk::Address,
        mode: u32,
        accounts: soroban_sdk::Vec<soroban_sdk::Address>,
    ) -> Result<(), ContractError> {
        let caller_kernel = governance_caller(&env, &caller)?;

        let mut kernel_accounts = Vec::with_capacity(accounts.len() as usize);
        for account in accounts.iter() {
            kernel_accounts.push(kernel_address_from_sdk(&env, &account));
        }

        let mut restrictions = Some(match mode {
            0 => None,
            1 => Some(Restrictions::Paused),
            2 => Some(Restrictions::Blacklist(kernel_accounts)),
            3 => Some(Restrictions::Whitelist(kernel_accounts)),
            _ => return Err(ContractError::InvalidInput),
        });

        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let next_restrictions = restrictions
                .take()
                .ok_or_else(|| RuntimeError::invalid_state("restrictions already consumed"))?;
            vault.set_restrictions(caller_kernel, next_restrictions)?;
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        emit_admin_event(&env, symbol_short!("s_rstrct"));
        Ok(())
    }

    pub fn set_sentinel(
        env: Env,
        caller: soroban_sdk::Address,
        sentinel: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        require_governance(&env, &caller)?;
        env.storage()
            .instance()
            .set(&VaultDataKey::Sentinel, &sentinel);
        emit_admin_event(&env, symbol_short!("s_sntnl"));
        Ok(())
    }

    pub fn set_guardians(
        env: Env,
        caller: soroban_sdk::Address,
        guardians: soroban_sdk::Vec<soroban_sdk::Address>,
    ) -> Result<(), ContractError> {
        require_governance(&env, &caller)?;
        env.storage()
            .instance()
            .set(&VaultDataKey::Guardians, &guardians);
        emit_admin_event(&env, symbol_short!("s_guards"));
        Ok(())
    }

    pub fn set_allocators(
        env: Env,
        caller: soroban_sdk::Address,
        allocators: soroban_sdk::Vec<soroban_sdk::Address>,
    ) -> Result<(), ContractError> {
        require_governance(&env, &caller)?;
        env.storage()
            .instance()
            .set(&VaultDataKey::Allocators, &allocators);
        emit_admin_event(&env, symbol_short!("s_allctr"));
        Ok(())
    }

    pub fn set_allowed_adapters(
        env: Env,
        caller: soroban_sdk::Address,
        adapters: soroban_sdk::Vec<soroban_sdk::Address>,
    ) -> Result<(), ContractError> {
        require_governance(&env, &caller)?;
        let queue_len = current_supply_queue_len(&env)?;
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
        emit_admin_event(&env, symbol_short!("s_adaptr"));
        Ok(())
    }

    pub fn set_skim_recipient(
        env: Env,
        caller: soroban_sdk::Address,
        recipient: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        require_governance(&env, &caller)?;
        set_config_address(&env, &VaultDataKey::SkimRecipient, &recipient);
        emit_admin_event(&env, symbol_short!("s_skimr"));
        Ok(())
    }

    pub fn skim(
        env: Env,
        caller: soroban_sdk::Address,
        token: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        require_governance(&env, &caller)?;
        let asset = get_config_address(&env, &VaultDataKey::AssetToken)?;
        let share = get_config_address(&env, &VaultDataKey::ShareToken)?;
        if token == asset || token == share {
            return Err(ContractError::InvalidInput);
        }

        let recipient = get_config_address(&env, &VaultDataKey::SkimRecipient)?;
        let client = soroban_sdk::token::Client::new(&env, &token);
        let balance = client.balance(&env.current_contract_address());
        if balance <= 0 {
            return Err(ContractError::InvalidState);
        }

        client.transfer(&env.current_contract_address(), &recipient, &balance);
        emit_admin_event(&env, symbol_short!("skim"));
        Ok(())
    }
    pub fn resync_idle_balance(env: Env) -> Result<(), ContractError> {
        let now_ns = ledger_timestamp_ns(&env)?;
        let cooldown_ns = 120_000_000_000u64; // 120 seconds
        let last_key = VaultDataKey::IdleResyncLastNs;
        let last_ns = env.storage().instance().get(&last_key).unwrap_or(0u64);
        if last_ns != 0 && now_ns.saturating_sub(last_ns) < cooldown_ns {
            return Err(ContractError::InvalidState);
        }
        let asset_token = get_config_address(&env, &VaultDataKey::AssetToken)?;
        let client = soroban_sdk::token::Client::new(&env, &asset_token);
        let actual_balance = to_u128(client.balance(&env.current_contract_address()))?;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let state = vault.state_mut()?;
            let before_idle = state.idle_assets;
            if !state.op_state.is_idle() {
                return Err(RuntimeError::invalid_state("only one op in flight"));
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
        with_contract_vault_contract_error(&env, &mut call)?;
        env.storage().instance().set(&last_key, &now_ns);
        emit_admin_event(&env, symbol_short!("resync"));
        Ok(())
    }

    pub fn cancel_migration(env: Env, caller: soroban_sdk::Address) -> Result<(), ContractError> {
        require_governance(&env, &caller)?;
        if !migration_in_progress(&env) {
            return Err(ContractError::InvalidState);
        }

        set_migration_in_progress(&env, false);
        emit_admin_event(&env, symbol_short!("cnc_migr"));
        Ok(())
    }

    pub fn config(
        env: Env,
    ) -> Result<
        (
            soroban_sdk::Address,
            soroban_sdk::Address,
            soroban_sdk::Address,
            soroban_sdk::Address,
        ),
        ContractError,
    > {
        Ok((
            get_config_address(&env, &VaultDataKey::Curator)?,
            get_config_address(&env, &VaultDataKey::Governance)?,
            get_config_address(&env, &VaultDataKey::AssetToken)?,
            get_config_address(&env, &VaultDataKey::ShareToken)?,
        ))
    }

    pub fn virtual_offsets(env: Env) -> Result<(i128, i128), ContractError> {
        let (virtual_shares, virtual_assets) = load_virtual_offsets(&env);
        Ok((to_i128(virtual_shares)?, to_i128(virtual_assets)?))
    }

    pub fn supply_queue(env: Env) -> Result<soroban_sdk::Vec<u32>, ContractError> {
        let mut queue = soroban_sdk::Vec::new(&env);
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            for target_id in vault.supply_queue_targets() {
                queue.push_back(target_id);
            }
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        Ok(queue)
    }

    pub fn is_paused(env: Env) -> Result<bool, ContractError> {
        let storage = SorobanStorage::new(&env);
        Ok(storage.is_paused())
    }

    pub fn vault_snapshot(env: Env) -> Result<(i128, i128, i128), ContractError> {
        Ok(query_vault_snapshot(&env))
    }

    pub fn fee_info(env: Env) -> Result<(i128, u64, i128, i128), ContractError> {
        let mut result: (i128, u64, i128, i128) = (0, 0, 0, 0);
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let anchor = vault.get_fee_anchor()?;
            let fees = vault.get_fees();
            result = (
                anchor.total_assets as i128,
                anchor.timestamp_ns,
                u128::from(fees.management.fee_wad) as i128,
                u128::from(fees.performance.fee_wad) as i128,
            );
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        Ok(result)
    }

    pub fn cap_groups(
        env: Env,
    ) -> Result<soroban_sdk::Vec<(soroban_sdk::String, i128, i128)>, ContractError> {
        let mut groups = soroban_sdk::Vec::new(&env);
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            for (id, rec) in vault.policy_state().cap_groups.iter() {
                let sdk_id = soroban_sdk::String::from_str(&env, &id.0);
                let abs_cap = rec.cap.absolute_cap.map(|c| c.get() as i128).unwrap_or(0);
                groups.push_back((sdk_id, abs_cap, rec.principal as i128));
            }
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        Ok(groups)
    }

    pub fn queue_tail(env: Env) -> Result<u64, ContractError> {
        let mut result = 0u64;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            result = vault.queue_tail()?;
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        Ok(result)
    }

    pub fn withdraw_status(env: Env) -> Result<(i64, i64, i64), ContractError> {
        let mut result: (i64, i64, i64) = (-1, -1, -1);
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            result.0 = vault
                .peek_next_pending_withdrawal_id()?
                .map(|id| id as i64)
                .unwrap_or(-1);
            result.1 = vault
                .get_withdrawing_op_id()?
                .map(|id| id as i64)
                .unwrap_or(-1);
            result.2 = vault
                .get_current_withdraw_request_id()?
                .map(|id| id as i64)
                .unwrap_or(-1);
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        Ok(result)
    }

    pub fn extend_ttl(env: Env) -> Result<(), ContractError> {
        extend_storage_ttl(&env);
        Ok(())
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

    pub fn is_migrating(env: Env) -> Result<bool, ContractError> {
        Ok(migration_in_progress(&env))
    }

    pub fn query_asset(env: Env) -> Result<soroban_sdk::Address, ContractError> {
        get_config_address(&env, &VaultDataKey::AssetToken)
    }

    pub fn total_assets(env: Env) -> Result<i128, ContractError> {
        Ok(query_vault_field(&env, |s| s.total_assets))
    }

    pub fn convert_to_shares(env: Env, assets: i128) -> Result<i128, ContractError> {
        if assets <= 0 {
            return Ok(0);
        }
        let (state, config) = load_state_and_config(&env)?;
        let assets_u128 = to_u128(assets)?;
        to_i128(convert_to_shares(&state, &config, assets_u128))
    }

    pub fn convert_to_assets(env: Env, shares: i128) -> Result<i128, ContractError> {
        if shares <= 0 {
            return Ok(0);
        }
        let (state, config) = load_state_and_config(&env)?;
        let shares_u128 = to_u128(shares)?;
        to_i128(convert_to_assets(&state, &config, shares_u128))
    }

    pub fn max_deposit(env: Env, _receiver: soroban_sdk::Address) -> Result<i128, ContractError> {
        max_deposit_or_mint(&env, false)
    }

    pub fn max_mint(env: Env, _receiver: soroban_sdk::Address) -> Result<i128, ContractError> {
        max_deposit_or_mint(&env, true)
    }

    pub fn max_withdraw(env: Env, owner: soroban_sdk::Address) -> Result<i128, ContractError> {
        max_withdraw_or_redeem(&env, &owner, false)
    }

    pub fn max_redeem(env: Env, owner: soroban_sdk::Address) -> Result<i128, ContractError> {
        max_withdraw_or_redeem(&env, &owner, true)
    }

    pub fn preview_deposit(env: Env, assets: i128) -> Result<i128, ContractError> {
        Self::convert_to_shares(env, assets)
    }

    pub fn preview_mint(env: Env, shares: i128) -> Result<i128, ContractError> {
        if shares <= 0 {
            return Ok(0);
        }
        let (state, config) = load_state_and_config(&env)?;
        let shares_u128 = to_u128(shares)?;
        to_i128(convert_to_assets_ceil(&state, &config, shares_u128))
    }

    pub fn preview_withdraw(env: Env, assets: i128) -> Result<i128, ContractError> {
        if assets <= 0 {
            return Ok(0);
        }
        let (state, config) = load_state_and_config(&env)?;
        let assets_u128 = to_u128(assets)?;
        to_i128(convert_to_shares_ceil(&state, &config, assets_u128))
    }

    pub fn preview_redeem(env: Env, shares: i128) -> Result<i128, ContractError> {
        Self::convert_to_assets(env, shares)
    }

    pub fn deposit(
        env: Env,
        assets: i128,
        receiver: soroban_sdk::Address,
        from: soroban_sdk::Address,
        operator: soroban_sdk::Address,
    ) -> Result<i128, ContractError> {
        require_signed(&operator);
        if assets <= 0 {
            return Err(ContractError::InvalidInput);
        }
        Self::deposit_with_min(env, from, receiver, assets, 0)
    }

    pub fn mint(
        env: Env,
        shares: i128,
        receiver: soroban_sdk::Address,
        from: soroban_sdk::Address,
        operator: soroban_sdk::Address,
    ) -> Result<i128, ContractError> {
        require_signed(&operator);
        if shares <= 0 {
            return Err(ContractError::InvalidInput);
        }
        let (state, config) = load_state_and_config(&env)?;
        let shares_u128 = to_u128(shares)?;
        let assets_needed = convert_to_assets_ceil(&state, &config, shares_u128);
        let assets_i128 = to_i128(assets_needed)?;
        Self::deposit_with_min(env, from, receiver, assets_i128, shares)?;
        Ok(assets_i128)
    }

    pub fn withdraw(
        env: Env,
        assets: i128,
        receiver: soroban_sdk::Address,
        owner: soroban_sdk::Address,
        operator: soroban_sdk::Address,
    ) -> Result<i128, ContractError> {
        let mut result: Option<i128> = None;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            result = Some(vault.atomic_withdraw(
                &env,
                assets,
                receiver.clone(),
                owner.clone(),
                operator.clone(),
            )?);
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        Ok(result.unwrap_or(0))
    }

    pub fn redeem(
        env: Env,
        shares: i128,
        receiver: soroban_sdk::Address,
        owner: soroban_sdk::Address,
        operator: soroban_sdk::Address,
    ) -> Result<i128, ContractError> {
        let mut result: Option<i128> = None;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            result = Some(vault.atomic_redeem(
                &env,
                shares,
                receiver.clone(),
                owner.clone(),
                operator.clone(),
            )?);
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        Ok(result.unwrap_or(0))
    }
}
