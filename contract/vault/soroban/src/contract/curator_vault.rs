use super::helpers::{
    contract_error, invalid_state_error, kernel_address_from_sdk, require_signed,
    transition_to_runtime,
};
use super::*;

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
        self.state = Some(match self.storage.load_state()? {
            Some(versioned) => versioned.state,
            None => VaultState::default(),
        });
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
            let versioned = VersionedState::new(state);
            let result = self.storage.save_state(&versioned);
            self.state = Some(versioned.state);
            result
        } else {
            Ok(())
        }
    }

    pub(crate) fn authorize(&self, kind: ActionKind, caller: Address) -> Result<(), RuntimeError> {
        self.auth.authorize(kind, caller, None)?;
        Ok(())
    }

    fn reserve_op_id(state: &mut VaultState) -> Result<u64, RuntimeError> {
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
            None => Err(RuntimeError::storage_error("vault state not loaded")),
        }
    }

    #[inline]
    pub fn state_mut(&mut self) -> Result<&mut VaultState, RuntimeError> {
        match self.state.as_mut() {
            Some(state) => Ok(state),
            None => Err(RuntimeError::storage_error("vault state not loaded")),
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
            return Err(RuntimeError::contract_error(
                "vault address mismatch for effect mapping",
            ));
        }
        self.register_address(vault_kernel, vault_sdk.clone())?;
        self.register_address(ESCROW_ADDRESS, vault_sdk)?;
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
            .ok_or_else(|| RuntimeError::storage_error("vault state not loaded"))?;
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
            return Err(contract_error("vault is paused"));
        }

        let summary = self.apply_kernel_action(
            KernelAction::Deposit {
                owner: caller,
                receiver,
                assets_in: assets,
                min_shares_out,
                now_ns,
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
            return Err(contract_error("no shares in vault"));
        }

        let request_id = state.withdraw_queue.next_pending_withdrawal_id;
        self.apply_kernel_action(
            KernelAction::RequestWithdraw {
                owner: caller,
                receiver,
                shares,
                min_assets_out,
                now_ns,
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
                return Err(contract_error(
                    "vault not in idle or withdrawing state for withdrawal",
                ));
            }
        }
        if self.state()?.op_state.is_idle() {
            let step_summary =
                self.apply_kernel_action(KernelAction::ExecuteWithdraw { now_ns }, now_ns)?;
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
        let now_ns = ledger_timestamp_ns(env)
            .map_err(|_| RuntimeError::invalid_input("timestamp overflow"))?;

        let fees_active = !self.config.fees.management.fee_wad.is_zero()
            || !self.config.fees.performance.fee_wad.is_zero();
        if fees_active && now_ns > self.state()?.fee_anchor.timestamp_ns {
            let _ = self.apply_kernel_action(KernelAction::RefreshFees { now_ns }, now_ns)?;
        }

        Ok((owner_kernel, receiver_kernel, operator_kernel, now_ns))
    }

    fn atomic_payout(
        &mut self,
        owner: Address,
        receiver: Address,
        operator: Address,
        amount: u128,
        kind: AtomicPayoutKind,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        self.apply_kernel_action(
            KernelAction::AtomicWithdraw {
                owner,
                receiver,
                operator,
                amount,
                kind,
                now_ns,
            },
            now_ns,
        )
    }

    #[inline(never)]
    pub fn atomic_withdraw(
        &mut self,
        env: &Env,
        assets: i128,
        receiver: SdkAddress,
        owner: SdkAddress,
        operator: SdkAddress,
    ) -> Result<i128, RuntimeError> {
        if assets <= 0 {
            return Err(RuntimeError::invalid_input("amount must be > 0"));
        }

        let (owner_kernel, receiver_kernel, operator_kernel, now_ns) =
            self.prepare_atomic_call(env, &receiver, &owner, &operator)?;

        let burned = self.atomic_payout(
            owner_kernel,
            receiver_kernel,
            operator_kernel,
            to_u128(assets).map_err(|_| RuntimeError::invalid_input("invalid assets"))?,
            AtomicPayoutKind::Withdraw,
            now_ns,
        )?;
        Ok(to_i128(burned.shares_burned)
            .map_err(|_| RuntimeError::invalid_input("burn overflow"))?)
    }

    #[inline(never)]
    pub fn atomic_redeem(
        &mut self,
        env: &Env,
        shares: i128,
        receiver: SdkAddress,
        owner: SdkAddress,
        operator: SdkAddress,
    ) -> Result<i128, RuntimeError> {
        if shares <= 0 {
            return Err(RuntimeError::invalid_input("amount must be > 0"));
        }

        let (owner_kernel, receiver_kernel, operator_kernel, now_ns) =
            self.prepare_atomic_call(env, &receiver, &owner, &operator)?;

        let summary = self.atomic_payout(
            owner_kernel,
            receiver_kernel,
            operator_kernel,
            to_u128(shares).map_err(|_| RuntimeError::invalid_input("invalid shares"))?,
            AtomicPayoutKind::Redeem,
            now_ns,
        )?;
        Ok(to_i128(summary.assets_transferred)
            .map_err(|_| RuntimeError::invalid_input("asset overflow"))?)
    }

    #[inline(never)]
    fn complete_withdrawal_from_idle(
        &mut self,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        let (pending_owner, pending_receiver, pending_escrow, pending_expected) = {
            let (_, p) = self
                .state()?
                .withdraw_queue
                .head()
                .ok_or_else(|| contract_error("withdraw queue empty"))?;
            (p.owner, p.receiver, p.escrow_shares, p.expected_assets)
        };

        let withdraw_op_id = match &self.state()?.op_state {
            OpState::Withdrawing(w) => {
                if pending_owner != w.owner
                    || pending_receiver != w.receiver
                    || pending_escrow != w.escrow_shares
                {
                    return Err(contract_error("withdrawal queue head mismatch"));
                }
                w.op_id
            }
            _ => return Err(contract_error("withdrawal not in progress")),
        };

        let available_assets = self.state()?.idle_assets;
        if available_assets < pending_expected && available_assets < MIN_WITHDRAWAL_ASSETS {
            return Ok(EffectSummary::new());
        }
        let Some(idle_settlement) =
            compute_idle_settlement(pending_escrow, pending_expected, available_assets)
        else {
            return Ok(EffectSummary::new());
        };

        let assets_out = idle_settlement.assets_out;
        if assets_out == 0 {
            return Ok(EffectSummary::new());
        }
        let burn_shares = idle_settlement.settlement.to_burn;
        let refund_shares = idle_settlement.settlement.refund;
        let op_id = withdraw_op_id;

        let collected = {
            let op_state = mem::take(&mut self.state_mut()?.op_state);
            transition_to_runtime(withdrawal_settled(op_state, op_id, assets_out, burn_shares))?
        };
        let ctx = self.effect_context(now_ns);
        self.ensure_effect_addresses_mapped(&collected.effects, &ctx)?;
        let mut summary = self.interpreter.execute_effects(&collected.effects, &ctx)?;
        self.state_mut()?.op_state = collected.new_state;

        let payout = match &self.state()?.op_state {
            OpState::Payout(state) => state,
            _ => return Err(contract_error("expected payout state after withdrawal")),
        };

        let transfer_effects = [KernelEffect::TransferAssets {
            to: payout.receiver,
            amount: assets_out,
        }];
        self.ensure_effect_addresses_mapped(&transfer_effects, &ctx)?;
        let transfer_summary = self.interpreter.execute_effects(&transfer_effects, &ctx)?;
        summary.merge(transfer_summary);

        let state = self.state_mut()?;
        state.idle_assets = state
            .idle_assets
            .checked_sub(assets_out)
            .ok_or_else(|| invalid_state_error("idle_assets underflow on withdrawal"))?;
        state.total_assets = state
            .idle_assets
            .checked_add(state.external_assets)
            .ok_or_else(|| invalid_state_error("total_assets overflow on withdrawal"))?;

        let settle_summary = self.apply_kernel_action(
            KernelAction::SettlePayout {
                op_id,
                outcome: PayoutOutcome::Success {
                    burn_shares,
                    refund_shares,
                },
            },
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
        match delta {
            AllocationDelta::Supply(d) => {
                if d.amount == 0 {
                    return Err(RuntimeError::invalid_input("amount must be > 0"));
                }

                let plan = vec![(d.market.into(), d.amount)];
                let op_id = self.begin_allocation_internal(caller, &plan, 0)?;
                {
                    let next_principal = self
                        .policy_state()
                        .principal_for(d.market)
                        .checked_add(d.amount)
                        .ok_or_else(|| invalid_state_error("principal overflow on supply"))?;
                    let policy = self.policy_state_mut();
                    policy.set_principal(d.market, next_principal);
                    policy.refresh_cap_group_principals();
                }
                self.sync_external_assets(caller, op_id, self.policy_state().external_assets(), 0)?;
                self.finish_allocation_internal(caller, op_id, 0)?;
                self.storage.save_policy_state(&self.policy_state)?;
                Ok(AllocationResult {
                    op_id,
                    new_external_assets: self.state()?.external_assets,
                    summary: EffectSummary::new(),
                })
            }
            AllocationDelta::Withdraw(d) => {
                if d.amount == 0 {
                    return Err(RuntimeError::invalid_input("amount must be > 0"));
                }

                let op_id = self.begin_allocation_withdraw_internal(caller, d.market.into(), 0)?;
                {
                    let next_principal = self
                        .policy_state()
                        .principal_for(d.market)
                        .checked_sub(d.amount)
                        .ok_or_else(|| invalid_state_error("principal underflow on withdraw"))?;
                    let policy = self.policy_state_mut();
                    policy.set_principal(d.market, next_principal);
                    policy.refresh_cap_group_principals();
                }
                {
                    let state = self.state_mut()?;
                    state.idle_assets = state
                        .idle_assets
                        .checked_add(d.amount)
                        .ok_or_else(|| invalid_state_error("idle_assets overflow on withdraw"))?;
                }
                let new_external = self.sync_external_assets(
                    caller,
                    op_id,
                    self.policy_state().external_assets(),
                    0,
                )?;
                self.finish_allocation_internal(caller, op_id, 0)?;
                self.storage.save_policy_state(&self.policy_state)?;
                Ok(AllocationResult {
                    op_id,
                    new_external_assets: new_external,
                    summary: EffectSummary::new(),
                })
            }
        }
    }

    pub(crate) fn begin_allocation_internal(
        &mut self,
        caller: Address,
        plan: &[(TargetId, u128)],
        now_ns: u64,
    ) -> Result<u64, RuntimeError> {
        self.authorize(ActionKind::BeginAllocating, caller)?;
        let op_id = self.state()?.next_op_id;
        self.apply_kernel_action(
            KernelAction::begin_allocating(op_id, plan.to_vec(), now_ns),
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
        self.authorize(ActionKind::BeginAllocating, caller)?;
        let op_id = self.state()?.next_op_id;
        self.apply_kernel_action(
            KernelAction::begin_allocating(op_id, vec![(market, 0)], now_ns),
            now_ns,
        )?;
        Ok(op_id)
    }

    pub(crate) fn finish_allocation_internal(
        &mut self,
        caller: Address,
        op_id: u64,
        now_ns: u64,
    ) -> Result<(), RuntimeError> {
        self.authorize(ActionKind::FinishAllocating, caller)?;
        self.apply_kernel_action(KernelAction::finish_allocating(op_id, now_ns), now_ns)?;
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

    #[cfg(any(test, feature = "testutils"))]
    pub fn begin_allocating(
        &mut self,
        caller: Address,
        plan: Vec<(TargetId, u128)>,
        current_ns: u64,
    ) -> Result<u64, RuntimeError> {
        let filtered_plan = self
            .policy_state
            .locks
            .filter_allocation_plan(&plan, current_ns);

        self.authorize(ActionKind::BeginAllocating, caller)?;
        let op_id = {
            let state = self.state_mut()?;

            let alloc_total: u128 = filtered_plan.iter().map(|(_, amt)| *amt).sum();
            if alloc_total > state.idle_assets {
                return Err(RuntimeError::insufficient_balance(
                    state.idle_assets,
                    alloc_total,
                ));
            }

            let op_id = Self::reserve_op_id(state)?;
            state.idle_assets -= alloc_total;
            state.sync_total_assets();

            let result = transition_to_runtime(start_allocation(
                mem::take(&mut state.op_state),
                filtered_plan,
                op_id,
            ))?;
            state.op_state = result.new_state;
            op_id
        };
        self.save_state()?;
        Ok(op_id)
    }

    #[cfg(any(test, feature = "testutils"))]
    pub fn finish_allocating(
        &mut self,
        caller: Address,
        op_id: u64,
    ) -> Result<AllocationResult, RuntimeError> {
        self.authorize(ActionKind::FinishAllocating, caller)?;
        let result = {
            let state = self.state_mut()?;
            let transition_result = transition_to_runtime(complete_allocation(
                mem::take(&mut state.op_state),
                op_id,
                None,
            ))?;
            state.op_state = transition_result.new_state;
            AllocationResult {
                op_id,
                new_external_assets: state.external_assets,
                summary: EffectSummary::new(),
            }
        };
        self.save_state()?;
        Ok(result)
    }

    pub fn begin_refreshing(
        &mut self,
        caller: Address,
        plan: Vec<TargetId>,
        current_ns: u64,
    ) -> Result<u64, RuntimeError> {
        self.authorize(ActionKind::BeginRefreshing, caller)?;
        let filtered_plan = self
            .policy_state
            .locks
            .build_refresh_plan_with_locks(&plan, current_ns);

        if filtered_plan.is_empty() {
            return Err(RuntimeError::invalid_input("empty refresh plan"));
        }

        let op_id = self.state()?.next_op_id;
        self.apply_kernel_action(
            KernelAction::begin_refreshing(op_id, filtered_plan, current_ns),
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
        let result = {
            let markets_refreshed = match &self.state()?.op_state {
                OpState::Refreshing(refresh) => refresh.plan.len() as u32,
                _ => 0,
            };
            self.apply_kernel_action(KernelAction::finish_refreshing(op_id, now_ns), now_ns)?;

            RefreshResult {
                op_id,
                markets_refreshed,
                new_external_assets: self.state()?.external_assets,
            }
        };
        Ok(result)
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
            KernelAction::sync_external_assets(new_external_assets, op_id, now_ns),
            now_ns,
        )?;
        Ok(self.state()?.external_assets)
    }

    pub(crate) fn rebalance_withdraw(
        &mut self,
        caller: Address,
        amount: u128,
        now_ns: u64,
    ) -> Result<u128, RuntimeError> {
        self.authorize(ActionKind::RebalanceWithdraw, caller)?;
        self.apply_kernel_action(KernelAction::rebalance_withdraw(amount, now_ns), now_ns)?;
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
            .cap_groups
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
            if entries
                .iter()
                .any(|entry: &SupplyQueueEntry| entry.target_id == target_id)
            {
                return Err(RuntimeError::invalid_input(
                    "duplicate market in supply queue",
                ));
            }
            entries.push(SupplyQueueEntry::new(target_id, 1));
        }

        self.policy_state.supply_queue = SupplyQueue::from(entries);
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

        let current_cap = self.policy_state.markets.get(&market_id).map(|m| m.cap);
        let decision = TimelockDecision::from_cap_change(current_cap, new_cap)
            .map_err(|_| RuntimeError::invalid_input("cap unchanged"))?;
        if matches!(decision, TimelockDecision::Timelocked) {
            return Err(RuntimeError::invalid_input(
                "cap increase or new market requires timelock",
            ));
        }

        let Some(config) = self.policy_state.markets.get_mut(&market_id) else {
            return Err(RuntimeError::invalid_input("market not found"));
        };
        config.cap = new_cap;

        self.storage.save_policy_state(&self.policy_state)?;
        Ok(())
    }

    pub fn remove_market(
        &mut self,
        caller: Address,
        market_id: TargetId,
    ) -> Result<(), RuntimeError> {
        self.auth.authorize(ActionKind::PolicyAdmin, caller, None)?;

        let principal = self.policy_state.principal_for(market_id);
        let Some(config) = self.policy_state.markets.get_mut(&market_id) else {
            return Err(RuntimeError::invalid_input("market not found"));
        };
        if config.cap > 0 {
            return Err(RuntimeError::invalid_input(
                "cannot remove market with non-zero cap",
            ));
        }
        if !config.enabled {
            return Err(RuntimeError::invalid_input("market already removed"));
        }
        if TimelockDecision::from_requires_timelock(principal > 0).requires_timelock() {
            return Err(RuntimeError::invalid_input(
                "market with principal requires timelock",
            ));
        }

        config.enabled = false;
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
                    .cap_groups
                    .get(&cap_group_id)
                    .and_then(|record| record.cap.absolute_cap.map(NonZeroU128::get));
                let decision = TimelockDecision::from_cap_group_cap_change(current, new_cap)
                    .map_err(|_| RuntimeError::invalid_input("cap group cap unchanged"))?;
                if matches!(decision, TimelockDecision::Timelocked) {
                    return Err(RuntimeError::invalid_input(
                        "cap group cap increase requires timelock",
                    ));
                }

                let record = self
                    .policy_state
                    .cap_groups
                    .get_mut(&cap_group_id)
                    .ok_or_else(|| RuntimeError::invalid_input("cap group not found"))?;
                record.cap.absolute_cap = NonZeroU128::new(new_cap);
            }
            CapGroupUpdate::SetRelativeCap {
                cap_group_id,
                new_relative_cap_wad,
            } => {
                let proposed = Wad::from(new_relative_cap_wad);
                let current = self
                    .policy_state
                    .cap_groups
                    .get(&cap_group_id)
                    .and_then(|record| record.cap.relative_cap);
                let decision = TimelockDecision::from_relative_cap_change(current, proposed)
                    .map_err(|_| {
                        RuntimeError::invalid_input("invalid cap group relative cap change")
                    })?;
                if matches!(decision, TimelockDecision::Timelocked) {
                    return Err(RuntimeError::invalid_input(
                        "cap group relative cap increase requires timelock",
                    ));
                }

                let record = self
                    .policy_state
                    .cap_groups
                    .get_mut(&cap_group_id)
                    .ok_or_else(|| RuntimeError::invalid_input("cap group not found"))?;
                record.cap.relative_cap = if proposed.is_zero() {
                    None
                } else {
                    Some(proposed)
                };
            }
            CapGroupUpdate::SetMembership {
                market_id,
                cap_group_id,
            } => {
                let changed = {
                    let market = self
                        .policy_state
                        .markets
                        .get(&market_id)
                        .ok_or_else(|| RuntimeError::invalid_input("market not found"))?;
                    market.cap_group_id != cap_group_id
                };
                let _decision = TimelockDecision::from_membership_change(changed)
                    .map_err(|_| RuntimeError::invalid_input("membership unchanged"))?;

                if let Some(group_id) = cap_group_id.as_ref() {
                    if !self.policy_state.cap_groups.contains_key(group_id) {
                        self.policy_state
                            .cap_groups
                            .insert(group_id.clone(), CapGroupRecord::default());
                    }
                }

                let market = self
                    .policy_state
                    .markets
                    .get_mut(&market_id)
                    .ok_or_else(|| RuntimeError::invalid_input("market not found"))?;
                market.cap_group_id = cap_group_id;
                self.policy_state.refresh_cap_group_principals();
            }
        }

        self.storage.save_policy_state(&self.policy_state)?;
        Ok(())
    }

    pub fn supply_queue_targets(&self) -> Vec<TargetId> {
        self.policy_state
            .supply_queue
            .entries
            .iter()
            .map(|entry| entry.target_id)
            .collect()
    }
}
