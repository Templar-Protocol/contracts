use super::helpers::{
    contract_error, invalid_state_error, kernel_address_from_sdk, require_signed,
};
use super::*;
use templar_curator_primitives::policy::state::PolicyStateError;
use templar_vault_kernel::abort;
use templar_vault_kernel::state::op_state::AllocationPlanEntry;

#[derive(Clone, Copy)]
struct SupplyAllocationDecision {
    market: TargetId,
    amount: u128,
    observed_total_assets: u128,
}

#[derive(Clone, Copy)]
struct WithdrawAllocationDecision {
    market: TargetId,
    amount: u128,
}

enum AllocationDecision {
    Supply(SupplyAllocationDecision),
    Withdraw(WithdrawAllocationDecision),
}

struct RefreshPlanDecision {
    markets: Vec<TargetId>,
}

#[derive(Clone, Copy)]
struct RefreshCompletionSnapshot {
    markets_refreshed: u32,
}

pub struct CuratorVault<S, A, E>
where
    S: Storage,
    A: AuthAdapter,
    E: EffectInterpreter + AddressRegistrar,
{
    pub config: ContractConfig,
    pub storage: S,
    pub auth: A,
    pub interpreter: E,
    state: Option<VaultState>,
    policy_state: PolicyState,
    restrictions: Option<Restrictions>,
    paused: bool,
}

impl<S, A, E> CuratorVault<S, A, E>
where
    S: Storage,
    A: AuthAdapter,
    E: EffectInterpreter + AddressRegistrar,
{
    #[inline]
    #[must_use]
    pub fn new(config: ContractConfig, storage: S, auth: A, interpreter: E) -> Self {
        Self {
            config,
            storage,
            auth,
            interpreter,
            state: None,
            policy_state: PolicyState::default(),
            restrictions: None,
            paused: false,
        }
    }

    #[inline(never)]
    pub fn load_state(&mut self) -> Result<(), RuntimeError> {
        self.state = Some((self.storage.load_state()?).unwrap_or_default());
        self.paused = self.storage.load_paused()?;
        self.policy_state = self
            .storage
            .load_policy_state()?
            .unwrap_or_else(PolicyState::default);
        self.restrictions = self.storage.load_restrictions()?;
        Ok(())
    }

    pub fn register_address(
        &mut self,
        kernel_addr: Address,
        soroban_addr: SdkAddress,
    ) -> Result<(), RuntimeError> {
        self.storage.save_address(&kernel_addr, &soroban_addr)?;
        self.interpreter.register_address(kernel_addr, soroban_addr);
        Ok(())
    }

    pub fn save_state(&mut self) -> Result<(), RuntimeError> {
        if let Some(state) = self.state.take() {
            let result = self.storage.save_state(&state);
            self.state = Some(state);
            result
        } else {
            Ok(())
        }
    }

    pub(crate) fn authorize(&self, kind: ActionKind, caller: Address) -> Result<(), RuntimeError> {
        self.auth.authorize(kind, caller, None)?;
        Ok(())
    }

    pub(crate) fn reserve_op_id(state: &mut VaultState) -> Result<u64, RuntimeError> {
        let op_id = state.next_op_id;
        state.next_op_id = state
            .next_op_id
            .checked_add(1)
            .ok_or_else(|| invalid_state_error("op_id overflow"))?;
        Ok(op_id)
    }

    #[inline]
    pub fn state(&self) -> Result<&VaultState, RuntimeError> {
        match self.state.as_ref() {
            Some(state) => Ok(state),
            None => Err(RuntimeError::storage_error("")),
        }
    }

    #[inline]
    pub fn state_mut(&mut self) -> Result<&mut VaultState, RuntimeError> {
        match self.state.as_mut() {
            Some(state) => Ok(state),
            None => Err(RuntimeError::storage_error("")),
        }
    }

    fn effect_context(&self, now_ns: u64) -> EffectContext {
        EffectContext::new(
            now_ns,
            self.config.vault_address,
            self.config.asset_address,
            self.config.share_address,
        )
    }

    fn ensure_vault_mapped(&mut self, env: &Env) -> Result<(), RuntimeError> {
        let vault_sdk = env.current_contract_address();
        let vault_kernel = kernel_address_from_sdk(env, &vault_sdk);
        if vault_kernel != self.config.vault_address {
            return Err(RuntimeError::contract_error("address mismatch"));
        }
        self.register_address(vault_kernel, vault_sdk)?;
        Ok(())
    }

    fn register_sdk_address(
        &mut self,
        env: &Env,
        addr: &SdkAddress,
    ) -> Result<Address, RuntimeError> {
        let kernel_addr = kernel_address_from_sdk(env, addr);
        self.register_address(kernel_addr, addr.clone())?;
        Ok(kernel_addr)
    }

    fn kernel_config(&self) -> VaultConfig {
        VaultConfig {
            fees: self.config.fees,
            min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
            withdrawal_cooldown_ns: DEFAULT_COOLDOWN_NS,
            max_pending_withdrawals: MAX_PENDING as u32,
            paused: self.paused,
            virtual_shares: self.config.virtual_shares,
            virtual_assets: self.config.virtual_assets,
        }
    }

    #[inline(never)]
    fn apply_kernel_action(
        &mut self,
        action: KernelAction,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        let config = self.kernel_config();
        let restrictions = self.restrictions.as_ref();
        let state = self
            .state
            .take()
            .ok_or_else(|| RuntimeError::storage_error(""))?;
        let result = match transition_to_runtime(apply_action(
            state.clone(),
            &config,
            restrictions,
            &self.config.vault_address,
            action,
        )) {
            Ok(r) => r,
            Err(e) => {
                self.state = Some(state);
                return Err(e);
            }
        };

        let ctx = self.effect_context(now_ns);
        self.ensure_effect_addresses_mapped(&result.effects, &ctx)?;
        let summary = self.interpreter.execute_effects(&result.effects, &ctx)?;
        self.state = Some(result.state);
        self.save_state()?;
        Ok(summary)
    }

    #[inline(never)]
    fn ensure_effect_addresses_mapped(
        &mut self,
        effects: &[KernelEffect],
        ctx: &EffectContext,
    ) -> Result<(), RuntimeError> {
        for effect in effects {
            match effect {
                KernelEffect::MintShares { owner, .. } | KernelEffect::BurnShares { owner, .. } => {
                    self.ensure_mapped(owner)?;
                }
                KernelEffect::BurnSharesFrom { spender, owner, .. } => {
                    self.ensure_mapped(spender)?;
                    self.ensure_mapped(owner)?;
                }
                KernelEffect::TransferShares { from, to, .. } => {
                    self.ensure_mapped(from)?;
                    self.ensure_mapped(to)?;
                }
                KernelEffect::TransferAssets { to, .. } => {
                    self.ensure_mapped(&ctx.vault_address)?;
                    self.ensure_mapped(to)?;
                }
                KernelEffect::TransferAssetsFrom { from, to, .. } => {
                    self.ensure_mapped(from)?;
                    self.ensure_mapped(to)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn ensure_mapped(&mut self, addr: &Address) -> Result<(), RuntimeError> {
        if self.interpreter.has_address(addr) {
            return Ok(());
        }
        if let Some(soroban_addr) = self.storage.load_address(addr)? {
            self.interpreter.register_address(*addr, soroban_addr);
            return Ok(());
        }
        Err(RuntimeError::effect_failed("missing address mapping"))
    }

    #[inline(never)]
    pub fn deposit(
        &mut self,
        caller: Address,
        receiver: Address,
        assets: u128,
        min_shares_out: u128,
        now_ns: u64,
    ) -> Result<DepositResult, RuntimeError> {
        self.authorize(ActionKind::Deposit, caller)?;
        if self.paused {
            return Err(contract_error("paused"));
        }

        let summary = self.apply_kernel_action(
            KernelAction::Deposit {
                owner: caller,
                receiver,
                assets_in: assets,
                min_shares_out,
                now_ns: TimestampNs(now_ns),
            },
            now_ns,
        )?;

        let state = self.state()?;
        Ok(DepositResult {
            shares_minted: summary.shares_minted,
            total_shares: state.total_shares,
            total_assets: state.total_assets,
        })
    }

    /// Map vault + caller + receiver SDK addresses to kernel addresses.
    pub fn map_pair(
        &mut self,
        env: &Env,
        caller: &SdkAddress,
        receiver: &SdkAddress,
    ) -> Result<(Address, Address), RuntimeError> {
        self.ensure_vault_mapped(env)?;
        let caller_kernel = self.register_sdk_address(env, caller)?;
        let receiver_kernel = self.register_sdk_address(env, receiver)?;
        Ok((caller_kernel, receiver_kernel))
    }

    #[inline(never)]
    pub fn request_withdraw(
        &mut self,
        caller: Address,
        receiver: Address,
        shares: u128,
        min_assets_out: u128,
        now_ns: u64,
    ) -> Result<WithdrawRequestResult, RuntimeError> {
        self.authorize(ActionKind::RequestWithdraw, caller)?;

        let state = self.state()?;
        if state.total_shares == 0 {
            return Err(contract_error("no shares"));
        }

        let request_id = state.withdraw_queue.next_pending_withdrawal_id;
        self.apply_kernel_action(
            KernelAction::RequestWithdraw {
                owner: caller,
                receiver,
                shares,
                min_assets_out,
                now_ns: TimestampNs(now_ns),
            },
            now_ns,
        )?;

        Ok(WithdrawRequestResult {
            request_id,
            shares_escrowed: shares,
        })
    }

    #[inline(never)]
    pub fn execute_withdraw(
        &mut self,
        caller: Address,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        self.authorize(ActionKind::ExecuteWithdraw, caller)?;

        let mut summary = EffectSummary::new();

        {
            let op_state = &self.state()?.op_state;
            if !op_state.is_idle() && !op_state.is_withdrawing() {
                return Err(contract_error("not idle or withdrawing"));
            }
        }
        if self.state()?.op_state.is_idle() {
            let step_summary = self.apply_kernel_action(
                KernelAction::ExecuteWithdraw {
                    now_ns: TimestampNs(now_ns),
                },
                now_ns,
            )?;
            summary.merge(step_summary);
        }

        if self.state()?.op_state.is_withdrawing() {
            let settle_summary = self.complete_withdrawal_from_idle(now_ns)?;
            summary.merge(settle_summary);
        }

        Ok(summary)
    }

    /// Map vault + caller SDK address to kernel address.
    pub fn map_caller(&mut self, env: &Env, caller: &SdkAddress) -> Result<Address, RuntimeError> {
        self.ensure_vault_mapped(env)?;
        self.register_sdk_address(env, caller)
    }

    fn prepare_atomic_call(
        &mut self,
        env: &Env,
        receiver: &SdkAddress,
        owner: &SdkAddress,
        operator: &SdkAddress,
    ) -> Result<(Address, Address, Address, u64), RuntimeError> {
        require_signed(operator);
        self.ensure_vault_mapped(env)?;
        let owner_kernel = self.register_sdk_address(env, owner)?;
        let receiver_kernel = self.register_sdk_address(env, receiver)?;
        let operator_kernel = self.register_sdk_address(env, operator)?;
        let now_ns = ledger_timestamp_ns(env).map_err(|_| RuntimeError::invalid_input(""))?;

        let fees_active = !self.config.fees.management.fee_wad.is_zero()
            || !self.config.fees.performance.fee_wad.is_zero();
        if fees_active && now_ns > self.state()?.fee_anchor.timestamp_ns.as_u64() {
            let _ = self.apply_kernel_action(
                KernelAction::RefreshFees {
                    now_ns: TimestampNs(now_ns),
                },
                now_ns,
            )?;
        }

        Ok((owner_kernel, receiver_kernel, operator_kernel, now_ns))
    }

    fn atomic_withdraw_effects(
        &mut self,
        owner: Address,
        receiver: Address,
        operator: Address,
        assets_out: u128,
        max_shares_burned: u128,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        self.apply_kernel_action(
            KernelAction::AtomicWithdraw {
                owner,
                receiver,
                operator,
                assets_out,
                max_shares_burned,
                now_ns: TimestampNs(now_ns),
            },
            now_ns,
        )
    }

    fn atomic_redeem_effects(
        &mut self,
        owner: Address,
        receiver: Address,
        operator: Address,
        shares: u128,
        min_assets_out: u128,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        self.apply_kernel_action(
            KernelAction::AtomicRedeem {
                owner,
                receiver,
                operator,
                shares,
                min_assets_out,
                now_ns: TimestampNs(now_ns),
            },
            now_ns,
        )
    }

    #[inline(never)]
    pub fn atomic_withdraw(
        &mut self,
        env: &Env,
        assets: i128,
        max_shares_burned: i128,
        receiver: SdkAddress,
        owner: SdkAddress,
        operator: SdkAddress,
    ) -> Result<i128, RuntimeError> {
        if assets <= 0 {
            return Err(RuntimeError::invalid_input(""));
        }

        let (owner_kernel, receiver_kernel, operator_kernel, now_ns) =
            self.prepare_atomic_call(env, &receiver, &owner, &operator)?;

        let burned = self.atomic_withdraw_effects(
            owner_kernel,
            receiver_kernel,
            operator_kernel,
            to_u128(assets).map_err(|_| RuntimeError::invalid_input(""))?,
            to_u128(max_shares_burned).map_err(|_| RuntimeError::invalid_input(""))?,
            now_ns,
        )?;
        to_i128(burned.shares_burned).map_err(|_| RuntimeError::invalid_input(""))
    }

    #[inline(never)]
    pub fn atomic_redeem(
        &mut self,
        env: &Env,
        shares: i128,
        min_assets_out: i128,
        receiver: SdkAddress,
        owner: SdkAddress,
        operator: SdkAddress,
    ) -> Result<i128, RuntimeError> {
        if shares <= 0 {
            return Err(RuntimeError::invalid_input(""));
        }

        let (owner_kernel, receiver_kernel, operator_kernel, now_ns) =
            self.prepare_atomic_call(env, &receiver, &owner, &operator)?;

        let summary = self.atomic_redeem_effects(
            owner_kernel,
            receiver_kernel,
            operator_kernel,
            to_u128(shares).map_err(|_| RuntimeError::invalid_input(""))?,
            to_u128(min_assets_out).map_err(|_| RuntimeError::invalid_input(""))?,
            now_ns,
        )?;
        to_i128(summary.assets_transferred).map_err(|_| RuntimeError::invalid_input(""))
    }

    #[inline(never)]
    fn complete_withdrawal_from_idle(
        &mut self,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        let Some(idle_payout) = transition_to_runtime(plan_idle_payout(self.state()?))? else {
            return Ok(EffectSummary::new());
        };

        let assets_out = idle_payout.assets_out;
        if assets_out == 0 {
            return Ok(EffectSummary::new());
        }
        let burn_shares = idle_payout.burn_shares;
        let op_id = idle_payout.op_id;

        let collected = {
            let op_state = mem::take(&mut self.state_mut()?.op_state);
            transition_to_runtime(withdrawal_settled(op_state, op_id, assets_out, burn_shares))?
        };
        let ctx = self.effect_context(now_ns);
        self.ensure_effect_addresses_mapped(&collected.effects, &ctx)?;
        let mut summary = self.interpreter.execute_effects(&collected.effects, &ctx)?;
        self.state_mut()?.op_state = collected.new_state;

        if !matches!(self.state()?.op_state, OpState::Payout(_)) {
            return Err(contract_error("expected payout state after withdrawal"));
        }

        let transfer_effects = [KernelEffect::TransferAssets {
            to: idle_payout.receiver,
            amount: assets_out,
        }];
        self.ensure_effect_addresses_mapped(&transfer_effects, &ctx)?;
        let transfer_summary = self.interpreter.execute_effects(&transfer_effects, &ctx)?;
        summary.merge(transfer_summary);

        let settle_summary = self.apply_kernel_action(
            KernelAction::settle_payout(op_id, PayoutOutcome::Success),
            now_ns,
        )?;
        summary.merge(settle_summary);

        Ok(summary)
    }

    pub fn pause(&mut self, caller: Address, paused: bool) -> Result<(), RuntimeError> {
        self.authorize(ActionKind::Pause, caller)?;
        self.paused = paused;
        self.storage.save_paused(paused)?;
        Ok(())
    }

    pub fn set_restrictions(
        &mut self,
        caller: Address,
        restrictions: Option<Restrictions>,
    ) -> Result<(), RuntimeError> {
        self.authorize(ActionKind::SetRestrictions, caller)?;
        self.restrictions = restrictions;
        self.storage.save_restrictions(&self.restrictions)?;
        Ok(())
    }

    pub fn allocate(
        &mut self,
        caller: Address,
        delta: &AllocationDelta,
    ) -> Result<AllocationResult, RuntimeError> {
        match self.classify_allocation(delta)? {
            AllocationDecision::Supply(decision) => {
                self.execute_supply_allocation(caller, decision)
            }
            AllocationDecision::Withdraw(decision) => {
                self.execute_withdraw_allocation(caller, decision)
            }
        }
    }

    fn classify_allocation(
        &self,
        delta: &AllocationDelta,
    ) -> Result<AllocationDecision, RuntimeError> {
        match delta {
            AllocationDelta::Supply(delta) => {
                Self::require_positive_allocation_amount(delta.amount)?;

                let observed_total_assets = self
                    .policy_state()
                    .principal_for(delta.market)
                    .ok_or_else(|| invalid_state_error("unknown market principal on supply"))?
                    .checked_add(delta.amount)
                    .ok_or_else(|| invalid_state_error("principal overflow on supply"))?;

                Ok(AllocationDecision::Supply(SupplyAllocationDecision {
                    market: delta.market,
                    amount: delta.amount,
                    observed_total_assets,
                }))
            }
            AllocationDelta::Withdraw(delta) => {
                Self::require_positive_allocation_amount(delta.amount)?;

                Ok(AllocationDecision::Withdraw(WithdrawAllocationDecision {
                    market: delta.market,
                    amount: delta.amount,
                }))
            }
        }
    }

    fn execute_supply_allocation(
        &mut self,
        caller: Address,
        decision: SupplyAllocationDecision,
    ) -> Result<AllocationResult, RuntimeError> {
        let op_id = self.begin_allocation_internal(
            caller,
            &[AllocationPlanEntry::new(decision.market, decision.amount)],
            0,
        )?;
        let new_external_assets = self.complete_supply_allocation(
            caller,
            decision.market,
            decision.observed_total_assets,
            op_id,
            0,
        )?;
        Ok(Self::allocation_result(op_id, new_external_assets))
    }

    fn execute_withdraw_allocation(
        &mut self,
        caller: Address,
        decision: WithdrawAllocationDecision,
    ) -> Result<AllocationResult, RuntimeError> {
        let op_id = self.begin_allocation_withdraw_internal(caller, decision.market, 0)?;
        let new_external_assets =
            self.complete_withdraw_allocation(caller, decision.market, decision.amount, op_id, 0)?;
        Ok(Self::allocation_result(op_id, new_external_assets))
    }

    #[inline]
    fn require_positive_allocation_amount(amount: u128) -> Result<(), RuntimeError> {
        if amount == 0 {
            return Err(RuntimeError::invalid_input(""));
        }

        Ok(())
    }

    #[inline]
    fn allocation_result(op_id: u64, new_external_assets: u128) -> AllocationResult {
        AllocationResult {
            op_id,
            new_external_assets,
            summary: EffectSummary::new(),
        }
    }

    #[inline]
    fn reserve_authorized_op_id(
        &mut self,
        caller: Address,
        action: ActionKind,
    ) -> Result<u64, RuntimeError> {
        self.authorize(action, caller)?;
        let state = self.state_mut()?;
        Self::reserve_op_id(state)
    }

    fn classify_refresh_plan(
        &self,
        plan: &[TargetId],
        current_ns: u64,
    ) -> Result<RefreshPlanDecision, RuntimeError> {
        let markets = self
            .policy_state
            .leases()
            .excluding_leased_targets(plan, TimestampNs(current_ns));

        if markets.is_empty() {
            return Err(RuntimeError::invalid_input(""));
        }

        Ok(RefreshPlanDecision { markets })
    }

    #[inline]
    fn snapshot_refresh_completion(state: &VaultState) -> RefreshCompletionSnapshot {
        let markets_refreshed = state
            .op_state
            .as_refreshing()
            .map_or(0, |refreshing| refreshing.plan.len() as u32);

        RefreshCompletionSnapshot { markets_refreshed }
    }

    #[inline]
    fn refresh_result(
        op_id: u64,
        markets_refreshed: u32,
        new_external_assets: u128,
    ) -> RefreshResult {
        RefreshResult {
            op_id,
            markets_refreshed,
            new_external_assets,
        }
    }

    pub(crate) fn begin_allocation_internal(
        &mut self,
        caller: Address,
        plan: &[AllocationPlanEntry],
        now_ns: u64,
    ) -> Result<u64, RuntimeError> {
        let op_id = self.reserve_authorized_op_id(caller, ActionKind::BeginAllocating)?;
        self.apply_kernel_action(
            KernelAction::begin_allocating(op_id, plan.to_vec(), TimestampNs(now_ns)),
            now_ns,
        )?;
        Ok(op_id)
    }

    pub(crate) fn begin_allocation_withdraw_internal(
        &mut self,
        caller: Address,
        market: TargetId,
        now_ns: u64,
    ) -> Result<u64, RuntimeError> {
        let op_id = self.reserve_authorized_op_id(caller, ActionKind::BeginAllocating)?;
        self.apply_kernel_action(
            KernelAction::begin_allocating(
                op_id,
                vec![AllocationPlanEntry::new(market, 0)],
                TimestampNs(now_ns),
            ),
            now_ns,
        )?;
        Ok(op_id)
    }

    fn update_market_principal(&mut self, market: TargetId, principal: u128) {
        let policy = self.policy_state_mut();
        policy
            .set_principal(market, principal)
            .unwrap_or_else(|_| abort!("market principal failed"));
    }

    pub(crate) fn complete_supply_allocation(
        &mut self,
        caller: Address,
        market: TargetId,
        observed_total_assets: u128,
        op_id: u64,
        now_ns: u64,
    ) -> Result<u128, RuntimeError> {
        self.update_market_principal(market, observed_total_assets);
        let new_external = self.sync_external_assets(
            caller,
            op_id,
            self.policy_state().external_assets()?,
            now_ns,
        )?;
        self.finish_allocation_internal(caller, op_id, now_ns)?;
        self.storage.save_policy_state(&self.policy_state)?;
        Ok(new_external)
    }

    pub(crate) fn complete_withdraw_allocation(
        &mut self,
        caller: Address,
        market: TargetId,
        realized_amount: u128,
        op_id: u64,
        now_ns: u64,
    ) -> Result<u128, RuntimeError> {
        let next_principal = self
            .policy_state()
            .principal_for(market)
            .ok_or_else(|| invalid_state_error("unknown market principal on withdraw"))?
            .checked_sub(realized_amount)
            .ok_or_else(|| invalid_state_error("principal underflow on withdraw"))?;
        self.update_market_principal(market, next_principal);
        let new_external = self.rebalance_withdraw(caller, op_id, realized_amount, now_ns)?;
        self.finish_allocation_internal(caller, op_id, now_ns)?;
        self.storage.save_policy_state(&self.policy_state)?;
        Ok(new_external)
    }

    #[inline]
    fn classify_refreshed_positions(
        refreshed_positions: &[(TargetId, u128)],
    ) -> Vec<(TargetId, u128)> {
        refreshed_positions.to_vec()
    }

    fn validate_refreshed_positions_against_plan(
        &self,
        refreshed_positions: &[(TargetId, u128)],
    ) -> Result<(), RuntimeError> {
        let refreshing = self
            .state()?
            .op_state
            .as_refreshing()
            .ok_or_else(|| invalid_state_error(""))?;

        for (market, _) in refreshed_positions {
            if !refreshing.plan.contains(market) {
                return Err(RuntimeError::invalid_input(""));
            }
        }

        Ok(())
    }

    fn apply_refreshed_positions(&mut self, refreshed_positions: &[(TargetId, u128)]) {
        let policy = self.policy_state_mut();
        for &(market, total_assets) in refreshed_positions {
            policy
                .set_principal(market, total_assets)
                .unwrap_or_else(|_| abort!("refresh principal failed"));
        }
    }

    pub(crate) fn complete_refresh_with_positions(
        &mut self,
        caller: Address,
        refreshed_positions: &[(TargetId, u128)],
        op_id: u64,
        now_ns: u64,
    ) -> Result<RefreshResult, RuntimeError> {
        let refreshed_positions = Self::classify_refreshed_positions(refreshed_positions);
        self.validate_refreshed_positions_against_plan(&refreshed_positions)?;
        self.apply_refreshed_positions(&refreshed_positions);
        let new_external_assets = self.policy_state().external_assets()?;
        self.sync_external_assets(caller, op_id, new_external_assets, now_ns)?;
        let result = self.finish_refreshing(caller, op_id, now_ns)?;
        self.storage.save_policy_state(&self.policy_state)?;
        Ok(result)
    }

    pub(crate) fn finish_allocation_internal(
        &mut self,
        caller: Address,
        op_id: u64,
        now_ns: u64,
    ) -> Result<(), RuntimeError> {
        self.authorize(ActionKind::FinishAllocating, caller)?;
        self.apply_kernel_action(
            KernelAction::finish_allocating(op_id, TimestampNs(now_ns)),
            now_ns,
        )?;
        Ok(())
    }

    pub fn refresh_markets(
        &mut self,
        caller: Address,
        markets: Vec<TargetId>,
        now_ns: u64,
    ) -> Result<RefreshResult, RuntimeError> {
        let op_id = self.begin_refreshing(caller, markets, now_ns)?;
        self.finish_refreshing(caller, op_id, now_ns)
    }

    pub fn begin_refreshing(
        &mut self,
        caller: Address,
        plan: Vec<TargetId>,
        current_ns: u64,
    ) -> Result<u64, RuntimeError> {
        let decision = self.classify_refresh_plan(&plan, current_ns)?;
        let op_id = self.reserve_authorized_op_id(caller, ActionKind::BeginRefreshing)?;
        self.apply_kernel_action(
            KernelAction::begin_refreshing(op_id, decision.markets, TimestampNs(current_ns)),
            current_ns,
        )?;
        Ok(op_id)
    }

    pub fn finish_refreshing(
        &mut self,
        caller: Address,
        op_id: u64,
        now_ns: u64,
    ) -> Result<RefreshResult, RuntimeError> {
        self.authorize(ActionKind::FinishRefreshing, caller)?;
        let snapshot = Self::snapshot_refresh_completion(self.state()?);
        self.apply_kernel_action(
            KernelAction::finish_refreshing(op_id, TimestampNs(now_ns)),
            now_ns,
        )?;
        Ok(Self::refresh_result(
            op_id,
            snapshot.markets_refreshed,
            self.state()?.external_assets,
        ))
    }

    pub(crate) fn sync_external_assets(
        &mut self,
        caller: Address,
        op_id: u64,
        new_external_assets: u128,
        now_ns: u64,
    ) -> Result<u128, RuntimeError> {
        self.authorize(ActionKind::SyncExternalAssets, caller)?;
        self.apply_kernel_action(
            KernelAction::sync_external_assets(new_external_assets, op_id, TimestampNs(now_ns)),
            now_ns,
        )?;
        Ok(self.state()?.external_assets)
    }

    pub(crate) fn rebalance_withdraw(
        &mut self,
        caller: Address,
        op_id: u64,
        amount: u128,
        now_ns: u64,
    ) -> Result<u128, RuntimeError> {
        self.authorize(ActionKind::RebalanceWithdraw, caller)?;
        self.apply_kernel_action(
            KernelAction::rebalance_withdraw(op_id, amount, TimestampNs(now_ns)),
            now_ns,
        )?;
        Ok(self.state()?.external_assets)
    }

    #[inline]
    #[must_use]
    pub fn policy_state(&self) -> &PolicyState {
        &self.policy_state
    }

    #[inline]
    #[must_use]
    pub fn restrictions(&self) -> Option<&Restrictions> {
        self.restrictions.as_ref()
    }

    #[inline]
    pub fn policy_state_mut(&mut self) -> &mut PolicyState {
        &mut self.policy_state
    }

    pub fn get_fee_anchor(&self) -> Result<FeeAccrualAnchor, RuntimeError> {
        Ok(self.state()?.fee_anchor)
    }

    pub fn get_fees(&self) -> &FeesSpec {
        &self.config.fees
    }

    pub fn get_cap_groups(&self) -> Vec<(CapGroupId, CapGroupRecord)> {
        self.policy_state
            .cap_groups()
            .iter()
            .map(|(id, rec)| (id.clone(), rec.clone()))
            .collect()
    }

    pub fn queue_tail(&self) -> Result<u64, RuntimeError> {
        Ok(self.state()?.withdraw_queue.next_pending_withdrawal_id)
    }

    pub fn peek_next_pending_withdrawal_id(&self) -> Result<Option<u64>, RuntimeError> {
        Ok(self.state()?.withdraw_queue.head().map(|(id, _)| id))
    }

    pub fn get_withdrawing_op_id(&self) -> Result<Option<u64>, RuntimeError> {
        let state = self.state()?;
        match &state.op_state {
            OpState::Withdrawing(w) => Ok(Some(w.op_id)),
            _ => Ok(None),
        }
    }

    pub fn get_current_withdraw_request_id(&self) -> Result<Option<u64>, RuntimeError> {
        let state = self.state()?;
        match &state.op_state {
            OpState::Withdrawing(_) | OpState::Payout(_) => {
                Ok(Some(state.withdraw_queue.next_withdraw_to_execute))
            }
            _ => Ok(None),
        }
    }

    pub fn set_supply_queue(
        &mut self,
        caller: Address,
        target_ids: Vec<TargetId>,
    ) -> Result<(), RuntimeError> {
        self.auth.authorize(ActionKind::PolicyAdmin, caller, None)?;

        let mut entries = Vec::with_capacity(target_ids.len());
        for target_id in target_ids {
            let config = self
                .policy_state
                .market_config(target_id)
                .ok_or_else(|| RuntimeError::invalid_input(""))?;
            if !config.enabled {
                return Err(RuntimeError::invalid_input(""));
            }
            if config.cap == 0 {
                return Err(RuntimeError::invalid_input(""));
            }

            if entries
                .iter()
                .any(|entry: &SupplyQueueEntry| entry.target_id == target_id)
            {
                return Err(RuntimeError::invalid_input(
                    "duplicate market in supply queue",
                ));
            }
            entries.push(
                SupplyQueueEntry::new(target_id, 1).map_err(|_| RuntimeError::invalid_input(""))?,
            );
        }

        self.policy_state
            .replace_supply_queue(
                SupplyQueue::try_from_entries(entries, None)
                    .map_err(|_| RuntimeError::invalid_input(""))?,
            )
            .map_err(|_| RuntimeError::invalid_input(""))?;
        self.storage.save_policy_state(&self.policy_state)?;
        Ok(())
    }

    pub fn set_cap(
        &mut self,
        caller: Address,
        market_id: TargetId,
        new_cap: u128,
    ) -> Result<(), RuntimeError> {
        self.auth.authorize(ActionKind::PolicyAdmin, caller, None)?;

        let current_cap = self.policy_state.market_config(market_id).map(|m| m.cap);
        let decision = TimelockDecision::from_cap_change(current_cap, new_cap)
            .map_err(|_| RuntimeError::invalid_input(""))?;
        if matches!(decision, TimelockDecision::Timelocked) {
            return Err(RuntimeError::invalid_input(
                "cap increase or new market requires timelock",
            ));
        }

        self.policy_state
            .set_market_cap(market_id, new_cap)
            .map_err(|_| RuntimeError::invalid_input(""))?;

        self.storage.save_policy_state(&self.policy_state)?;
        Ok(())
    }

    pub fn apply_governance_cap(
        &mut self,
        caller: Address,
        market_id: TargetId,
        new_cap: u128,
    ) -> Result<(), RuntimeError> {
        self.auth.authorize(ActionKind::PolicyAdmin, caller, None)?;

        if self.policy_state.market_config(market_id).is_some() {
            self.policy_state
                .set_market_cap(market_id, new_cap)
                .map_err(|_| RuntimeError::invalid_input(""))?;
        } else {
            self.policy_state
                .set_market_config(market_id, MarketConfig::new(true, new_cap, None))
                .map_err(|_| RuntimeError::invalid_input(""))?;
        }

        self.storage.save_policy_state(&self.policy_state)?;
        Ok(())
    }

    pub fn remove_market(
        &mut self,
        caller: Address,
        market_id: TargetId,
    ) -> Result<(), RuntimeError> {
        self.auth.authorize(ActionKind::PolicyAdmin, caller, None)?;

        let principal = self.policy_state.principal_for(market_id).unwrap_or(0);
        let Some(config) = self.policy_state.market_config(market_id) else {
            return Err(RuntimeError::invalid_input(""));
        };
        if config.cap > 0 {
            return Err(RuntimeError::invalid_input(
                "cannot remove market with non-zero cap",
            ));
        }
        if !config.enabled {
            return Err(RuntimeError::invalid_input(""));
        }
        if TimelockDecision::from_requires_timelock(principal > 0).requires_timelock() {
            return Err(RuntimeError::invalid_input(
                "market with principal requires timelock",
            ));
        }

        let _ = self
            .policy_state
            .remove_market(market_id)
            .map_err(|_| RuntimeError::invalid_input(""))?;
        self.storage.save_policy_state(&self.policy_state)?;
        Ok(())
    }

    pub fn apply_governance_remove_market(
        &mut self,
        caller: Address,
        market_id: TargetId,
    ) -> Result<(), RuntimeError> {
        self.auth.authorize(ActionKind::PolicyAdmin, caller, None)?;

        let Some(config) = self.policy_state.market_config(market_id) else {
            return Err(RuntimeError::invalid_input(""));
        };
        if config.cap > 0 {
            return Err(RuntimeError::invalid_input(
                "cannot remove market with non-zero cap",
            ));
        }

        let _ = self
            .policy_state
            .remove_market(market_id)
            .map_err(|_| RuntimeError::invalid_input(""))?;
        self.storage.save_policy_state(&self.policy_state)?;
        Ok(())
    }

    #[inline(never)]
    pub fn update_cap_group(
        &mut self,
        caller: Address,
        update: CapGroupUpdate,
    ) -> Result<(), RuntimeError> {
        self.auth.authorize(ActionKind::PolicyAdmin, caller, None)?;

        match update {
            CapGroupUpdate::SetCap {
                cap_group_id,
                new_cap,
            } => {
                let current = self
                    .policy_state
                    .cap_group(&cap_group_id)
                    .and_then(|record| record.cap.absolute_cap());
                let decision = TimelockDecision::from_cap_group_cap_change(current, new_cap)
                    .map_err(|_| RuntimeError::invalid_input(""))?;
                if matches!(decision, TimelockDecision::Timelocked) {
                    return Err(RuntimeError::invalid_input(
                        "cap increase requires timelock",
                    ));
                }

                self.policy_state
                    .set_cap_group_absolute_cap(cap_group_id, new_cap);
            }
            CapGroupUpdate::SetRelativeCap {
                cap_group_id,
                new_relative_cap,
            } => {
                let proposed = new_relative_cap;
                let current = self
                    .policy_state
                    .cap_group(&cap_group_id)
                    .and_then(|record| record.cap.relative_cap());
                let decision = TimelockDecision::from_relative_cap_change(current, proposed)
                    .map_err(|_| RuntimeError::invalid_input(""))?;
                if matches!(decision, TimelockDecision::Timelocked) {
                    return Err(RuntimeError::invalid_input(
                        "cap increase requires timelock",
                    ));
                }

                self.policy_state
                    .set_cap_group_relative_cap(cap_group_id, proposed);
            }
            CapGroupUpdate::SetMembership {
                market_id,
                cap_group_id,
            } => {
                let market = self
                    .policy_state
                    .market_config(market_id)
                    .ok_or_else(|| RuntimeError::invalid_input(""))?;
                let _decision = TimelockDecision::from_membership_assignment_change(
                    market.cap_group_id.as_ref(),
                    cap_group_id.as_ref(),
                )
                .map_err(|_| RuntimeError::invalid_input(""))?;

                self.policy_state
                    .set_market_cap_group(market_id, cap_group_id)
                    .map_err(|error| match error {
                        PolicyStateError::UnknownCapGroup { .. }
                        | PolicyStateError::CapGroupInUse { .. }
                        | PolicyStateError::UnknownMarket { .. }
                        | PolicyStateError::PrincipalOverflow { .. }
                        | PolicyStateError::InvalidSupplyQueue { .. }
                        | PolicyStateError::SupplyQueueUnknownMarket { .. }
                        | PolicyStateError::SupplyQueueDisabledMarket { .. }
                        | PolicyStateError::SupplyQueueUnauthorizedMarket { .. } => {
                            RuntimeError::invalid_input("")
                        }
                    })?;
            }
        }

        self.storage.save_policy_state(&self.policy_state)?;
        Ok(())
    }

    pub fn apply_governance_cap_group_update(
        &mut self,
        caller: Address,
        update: CapGroupUpdate,
    ) -> Result<(), RuntimeError> {
        self.auth.authorize(ActionKind::PolicyAdmin, caller, None)?;

        match update {
            CapGroupUpdate::SetCap {
                cap_group_id,
                new_cap,
            } => {
                self.policy_state
                    .set_cap_group_absolute_cap(cap_group_id, new_cap);
            }
            CapGroupUpdate::SetRelativeCap {
                cap_group_id,
                new_relative_cap,
            } => {
                self.policy_state
                    .set_cap_group_relative_cap(cap_group_id, new_relative_cap);
            }
            CapGroupUpdate::SetMembership {
                market_id,
                cap_group_id,
            } => {
                self.policy_state
                    .set_market_cap_group(market_id, cap_group_id)
                    .map_err(|_| RuntimeError::invalid_input(""))?;
            }
        }

        self.storage.save_policy_state(&self.policy_state)?;
        Ok(())
    }

    pub fn supply_queue_targets(&self) -> Vec<TargetId> {
        self.policy_state
            .supply_queue()
            .entries()
            .iter()
            .map(|entry| entry.target_id)
            .collect()
    }
}
