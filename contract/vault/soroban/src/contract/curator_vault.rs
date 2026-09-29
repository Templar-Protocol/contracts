use super::helpers::{
    contract_error, invalid_state_error, kernel_address_from_sdk, require_signed,
};
use super::*;
use crate::storage::deposit::PendingStorage;
use soroban_sdk::{IntoVal, Symbol};
use templar_curator_primitives::policy::state::PolicyStateError;
use templar_soroban_shared_types::{
    EpochSnapshotReceipt, EpochStateViewReceipt, ReportMetadataReceipt, EPOCH_PHASE_CUTOFF,
    EPOCH_PHASE_OPEN, EPOCH_PHASE_SETTLED,
};
use templar_vault_kernel::abort;
use templar_vault_kernel::state::op_state::AllocationPlanEntry;
use templar_soroban_shared_types::CustodialValuationView;
use templar_vault_kernel::{
    EpochId, EpochPhase, PendingDeposit, TimestampNs, ValuationReportRef,
};

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
            withdrawal_cooldown_ns: self.config.withdrawal_cooldown_ns,
            max_pending_withdrawals: SOROBAN_MAX_PENDING_WITHDRAWALS,
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
        let (_, summary) = self.apply_kernel_action_effects(action, now_ns)?;
        Ok(summary)
    }

    #[inline(never)]
    fn apply_kernel_action_effects(
        &mut self,
        action: KernelAction,
        now_ns: u64,
    ) -> Result<(Vec<KernelEffect>, EffectSummary), RuntimeError> {
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
        Ok((result.effects, summary))
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

        let (effects, _) = self.apply_kernel_action_effects(
            KernelAction::Deposit {
                owner: caller,
                receiver,
                assets_in: assets,
                min_shares_out,
                now_ns: TimestampNs(now_ns),
            },
            now_ns,
        )?;
        let shares_minted = effects
            .iter()
            .find_map(|effect| match effect {
                KernelEffect::EmitEvent {
                    event:
                        templar_vault_kernel::effects::KernelEvent::DepositProcessed {
                            shares_out, ..
                        },
                } => Some(*shares_out),
                _ => None,
            })
            .unwrap_or(0);

        let state = self.state()?;
        Ok(DepositResult {
            shares_minted,
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
    pub fn refresh_fees(&mut self, now_ns: u64) -> Result<(), RuntimeError> {
        self.apply_kernel_action(
            KernelAction::RefreshFees {
                now_ns: TimestampNs(now_ns),
            },
            now_ns,
        )?;
        Ok(())
    }

    #[inline(never)]
    pub fn execute_withdraw(
        &mut self,
        caller: Address,
        now_ns: u64,
    ) -> Result<ExecuteWithdrawResult, RuntimeError> {
        self.authorize(ActionKind::ExecuteWithdraw, caller)?;

        let mut summary = EffectSummary::new();
        let mut completed = None;

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
            let settle_result = self.complete_withdrawal_from_idle(now_ns)?;
            let ExecuteWithdrawResult::Payout {
                summary: settle_summary,
                completed: settled,
            } = settle_result
            else {
                return Err(contract_error("expected withdrawal payout"));
            };
            summary.merge(settle_summary);
            completed = Some(settled);
        }

        Ok(match completed {
            Some(completed) => ExecuteWithdrawResult::Payout { summary, completed },
            None => ExecuteWithdrawResult::NoPayout { summary },
        })
    }

    #[inline(never)]
    pub fn abort_withdrawing(
        &mut self,
        caller: Address,
        op_id: u64,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        self.authorize(ActionKind::AbortWithdrawing, caller)?;
        self.apply_kernel_action(KernelAction::abort_withdrawing(op_id), now_ns)
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
    ) -> Result<(Address, Address, Address, u64, EffectSummary), RuntimeError> {
        require_signed(operator);
        self.ensure_vault_mapped(env)?;
        let owner_kernel = self.register_sdk_address(env, owner)?;
        let receiver_kernel = self.register_sdk_address(env, receiver)?;
        let operator_kernel = self.register_sdk_address(env, operator)?;
        let now_ns = ledger_timestamp_ns(env).map_err(|_| RuntimeError::invalid_input(""))?;

        let mut summary = EffectSummary::new();
        let fees_active = !self.config.fees.management.fee_wad.is_zero()
            || !self.config.fees.performance.fee_wad.is_zero();
        if fees_active && now_ns > self.state()?.fee_anchor.timestamp_ns.as_u64() {
            summary.merge(self.apply_kernel_action(
                KernelAction::RefreshFees {
                    now_ns: TimestampNs(now_ns),
                },
                now_ns,
            )?);
        }

        Ok((
            owner_kernel,
            receiver_kernel,
            operator_kernel,
            now_ns,
            summary,
        ))
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

        let (owner_kernel, receiver_kernel, operator_kernel, now_ns, mut summary) =
            self.prepare_atomic_call(env, &receiver, &owner, &operator)?;

        summary.merge(self.atomic_withdraw_effects(
            owner_kernel,
            receiver_kernel,
            operator_kernel,
            to_u128(assets).map_err(|_| RuntimeError::invalid_input(""))?,
            to_u128(max_shares_burned).map_err(|_| RuntimeError::invalid_input(""))?,
            now_ns,
        )?);
        to_i128(summary.shares_burned).map_err(|_| RuntimeError::invalid_input(""))
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

        let (owner_kernel, receiver_kernel, operator_kernel, now_ns, mut summary) =
            self.prepare_atomic_call(env, &receiver, &owner, &operator)?;

        summary.merge(self.atomic_redeem_effects(
            owner_kernel,
            receiver_kernel,
            operator_kernel,
            to_u128(shares).map_err(|_| RuntimeError::invalid_input(""))?,
            to_u128(min_assets_out).map_err(|_| RuntimeError::invalid_input(""))?,
            now_ns,
        )?);
        to_i128(summary.assets_transferred).map_err(|_| RuntimeError::invalid_input(""))
    }

    #[inline(never)]
    fn complete_withdrawal_from_idle(
        &mut self,
        now_ns: u64,
    ) -> Result<ExecuteWithdrawResult, RuntimeError> {
        let min_withdrawal_assets = self.kernel_config().min_withdrawal_assets;
        let idle_payout =
            transition_to_runtime(plan_idle_payout(self.state()?, min_withdrawal_assets))?;

        let assets_out = idle_payout.assets_out;
        let burn_shares = idle_payout.burn_shares;
        let op_id = idle_payout.op_id;
        let completed = CompletedWithdrawal {
            request_id: idle_payout.request_id,
            owner: idle_payout.owner,
            receiver: idle_payout.receiver,
            assets_out,
            shares_burned: burn_shares,
        };

        let ctx = self.effect_context(now_ns);
        let progressed = {
            let op_state = mem::take(&mut self.state_mut()?.op_state);
            transition_to_runtime(withdrawal_step_callback(op_state, op_id, assets_out))?
        };
        self.ensure_effect_addresses_mapped(&progressed.effects, &ctx)?;
        let mut summary = self.interpreter.execute_effects(&progressed.effects, &ctx)?;
        self.state_mut()?.op_state = progressed.new_state;

        let collected = {
            let op_state = mem::take(&mut self.state_mut()?.op_state);
            let vault = self.state()?;
            transition_to_runtime(withdrawal_settled(op_state, vault, op_id, min_withdrawal_assets))?
        };
        self.ensure_effect_addresses_mapped(&collected.effects, &ctx)?;
        summary.merge(self.interpreter.execute_effects(&collected.effects, &ctx)?);
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

        Ok(ExecuteWithdrawResult::Payout { summary, completed })
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

    fn set_policy_principal(
        policy: &mut PolicyState,
        market: TargetId,
        principal: u128,
        message: &'static str,
    ) -> Result<(), RuntimeError> {
        policy
            .set_principal(market, principal)
            .map_err(|_| invalid_state_error(message))
    }

    fn validate_supply_observation(
        policy: &PolicyState,
        market: TargetId,
        observed_total_assets: u128,
        supply_amount: u128,
    ) -> Result<(), RuntimeError> {
        let previous_principal = policy
            .principal_for(market)
            .ok_or_else(|| invalid_state_error("unknown market principal on supply"))?;
        let max_principal = previous_principal
            .checked_add(supply_amount)
            .ok_or_else(|| invalid_state_error("principal overflow on supply"))?;
        if observed_total_assets < previous_principal || observed_total_assets > max_principal {
            return Err(invalid_state_error("supply observation out of bounds"));
        }
        Ok(())
    }

    fn validate_refresh_observation(
        policy: &PolicyState,
        market: TargetId,
        observed_total_assets: u128,
    ) -> Result<(), RuntimeError> {
        let cap = policy
            .market_config(market)
            .ok_or_else(|| invalid_state_error("unknown refreshed market"))?
            .cap;
        if observed_total_assets > cap {
            return Err(invalid_state_error(
                "refresh observation exceeds market cap",
            ));
        }
        Ok(())
    }

    pub(crate) fn complete_supply_allocation(
        &mut self,
        caller: Address,
        market: TargetId,
        observed_total_assets: u128,
        op_id: u64,
        now_ns: u64,
    ) -> Result<u128, RuntimeError> {
        let allocation = self
            .state()?
            .op_state
            .as_allocating()
            .ok_or_else(|| invalid_state_error(""))?;
        let current_step = allocation
            .plan
            .get(allocation.index as usize)
            .ok_or_else(|| invalid_state_error("allocation step missing"))?;
        if current_step.target_id != market {
            return Err(RuntimeError::invalid_input(""));
        }
        Self::validate_supply_observation(
            self.policy_state(),
            market,
            observed_total_assets,
            current_step.amount,
        )?;
        let mut staged_policy = self.policy_state.clone();
        Self::set_policy_principal(
            &mut staged_policy,
            market,
            observed_total_assets,
            "market principal failed",
        )?;
        let new_external_assets = staged_policy.external_assets()?;
        let new_external = self.sync_external_assets(caller, op_id, new_external_assets, now_ns)?;
        self.finish_allocation_internal(caller, op_id, now_ns)?;
        self.policy_state = staged_policy;
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

    fn stage_refreshed_positions(
        &self,
        refreshed_positions: &[(TargetId, u128)],
    ) -> Result<PolicyState, RuntimeError> {
        let mut policy = self.policy_state.clone();
        for &(market, total_assets) in refreshed_positions {
            Self::validate_refresh_observation(&policy, market, total_assets)?;
            Self::set_policy_principal(
                &mut policy,
                market,
                total_assets,
                "refresh principal failed",
            )?;
        }
        Ok(policy)
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
        let staged_policy = self.stage_refreshed_positions(&refreshed_positions)?;
        let new_external_assets = staged_policy.external_assets()?;
        self.sync_external_assets(caller, op_id, new_external_assets, now_ns)?;
        let result = self.finish_refreshing(caller, op_id, now_ns)?;
        self.policy_state = staged_policy;
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
        self.set_supply_queue_authorized(target_ids)
    }

    pub fn set_supply_queue_authorized(
        &mut self,
        target_ids: Vec<TargetId>,
    ) -> Result<(), RuntimeError> {
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
        self.apply_governance_cap_authorized(market_id, new_cap)
    }

    pub fn apply_governance_cap_authorized(
        &mut self,
        market_id: TargetId,
        new_cap: u128,
    ) -> Result<(), RuntimeError> {
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
        self.apply_governance_remove_market_authorized(market_id)
    }

    pub fn apply_governance_remove_market_authorized(
        &mut self,
        market_id: TargetId,
    ) -> Result<(), RuntimeError> {
        let Some(config) = self.policy_state.market_config(market_id) else {
            return Err(RuntimeError::invalid_input(""));
        };
        let principal = self.policy_state.principal_for(market_id).unwrap_or(0);
        if config.cap > 0 {
            return Err(RuntimeError::invalid_input(
                "cannot remove market with non-zero cap",
            ));
        }
        if principal > 0 {
            return Err(RuntimeError::invalid_input(
                "cannot remove market with non-zero principal",
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
        self.apply_governance_cap_group_update_authorized(update)
    }

    pub fn apply_governance_cap_group_update_authorized(
        &mut self,
        update: CapGroupUpdate,
    ) -> Result<(), RuntimeError> {
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

/// Outcome of recording a pending-deposit liability for a receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingDepositResponse {
    pub request_id: u64,
    pub assets: i128,
}

/// Outcome of an owner-bound pending-deposit cancellation refund.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CancelPendingDepositResponse {
    pub request_id: u64,
    pub assets_refunded: i128,
}

/// Outcome of an epoch-intake cutoff for a receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeginEpochCutoffResponse {
    pub epoch_id: u64,
    pub cutoff_ns: u64,
}

/// Full settled-snapshot metadata for a settlement receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettleEpochResponse {
    pub epoch_id: u64,
    pub report_seq: u64,
    pub as_of_ns: u64,
    pub report_hash: [u8; 32],
    pub settlement_nav: i128,
    pub eligible_supply: i128,
    pub cutoff_ns: u64,
}

/// Outcome of a pending-deposit admission for a receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmitPendingDepositResponse {
    pub request_id: u64,
    pub shares_out: i128,
    pub assets_in: u128,
}

/// Outcome of an owner-bound withdrawal cancellation for a receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CancelPendingWithdrawalResponse {
    pub request_id: u64,
    pub shares_refunded: i128,
    pub epoch_id: u64,
}

/// Outcome of a governed one-time backed seed for a receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeedEpochSupplyResponse {
    pub assets_seeded: i128,
    pub shares_minted: i128,
}

impl<'a, A, E> CuratorVault<SorobanStorage<'a>, A, E>
where
    A: AuthAdapter,
    E: EffectInterpreter + AddressRegistrar,
{

    /// Record an epoch settlement freshness configuration. Storage rejects
    /// a zero bound, so a configured value can never silently disable the
    /// staleness law. Governance authorization is enforced by the proxy
    /// entrypoint; this path performs no kernel action.
    pub(crate) fn configure_epoch_settlement(
        &mut self,
        max_report_age_ns: u64,
    ) -> Result<(), RuntimeError> {
        if max_report_age_ns == 0 {
            return Err(RuntimeError::invalid_input("epoch freshness bound zero"));
        }
        self.storage.save_max_report_age_ns(max_report_age_ns)
    }

    /// Execute the governed one-time backed seed for a fresh epoch
    /// deployment.
    ///
    /// Governance authorization is enforced by the proxy entrypoint and is
    /// bound to the full command payload, matching the governance-only
    /// settlement-configuration operation. The asset is always the vault
    /// asset held in instance storage. Caller, amount, and receiver are
    /// bound by law and authorization.
    /// Governance first moves its own assets into vault custody through the
    /// asset contract's transfer entrypoint, signed by governance itself,
    /// because the classic asset contract authenticates only the source
    /// account and cannot be debited by proxy from inside this contract.
    /// This command observes custody before any mint law is dispatched:
    /// vault custody of the vault asset must equal the requested amount
    /// exactly, which rejects zero, partial, fee-skimmed, and excessive
    /// custody before any mint law is dispatched.
    ///
    /// Replay exclusion is derived from durable state, not a new field: the
    /// operation applies only while vault accounting is zero, the epoch is
    /// genesis with no cutoff, accepted report, or settlement snapshot, and
    /// the intake ledger has never recorded a request. Any intake record,
    /// cutoff, report, settlement, or prior seed leaves durable state that
    /// can never return to this pristine condition, so the operation can
    /// execute exactly once and never after any epoch or intake progression.
    /// Failure leaves no mint and no state progression. Pause semantics
    /// follow initialization and governance safety: the bootstrap remains
    /// available while paused.
    #[inline(never)]
    pub fn seed_epoch_supply(
        &mut self,
        env: &Env,
        _caller_sdk: &SdkAddress,
        receiver_sdk: &SdkAddress,
        assets_i128: i128,
    ) -> Result<SeedEpochSupplyResponse, RuntimeError> {
        self.ensure_vault_mapped(env)?;
        if !self.storage.epoch_mode_active() {
            return Err(invalid_state_error("epoch mode disabled"));
        }
        let assets =
            to_u128(assets_i128).map_err(|_| RuntimeError::invalid_input("seed assets"))?;
        if assets == 0 {
            return Err(RuntimeError::invalid_input("zero epoch seed"));
        }
        if self.state()?.total_assets != 0 || self.state()?.total_shares != 0 {
            return Err(invalid_state_error("epoch seed requires zero accounting"));
        }
        let epoch = self.storage.load_epoch_state()?;
        if epoch.phase != EpochPhase::Open
            || epoch.intake_epoch != EpochId::FIRST_SETTLEMENT
            || epoch.cutoff_ns.is_some()
            || epoch.last_settled.is_some()
        {
            return Err(invalid_state_error("epoch seed requires genesis state"));
        }
        if self.storage.pending_deposit_stats()?.count > 0 {
            return Err(invalid_state_error("epoch seed requires pristine intake"));
        }
        if self.storage.next_deposit_request_id()? > 1 {
            return Err(invalid_state_error("epoch seed blocked by prior intake"));
        }

        // Custody observation before mint law: the vault's own balance of
        // the vault asset must equal the amount exactly. The kernel law
        // re-checks pristine state and mints one-for-one against this
        // observed custody, so a seed can never mint synthetic shares.
        let asset_token = get_config_address(env, &VaultDataKey::AssetToken)
            .map_err(|_| RuntimeError::storage_error("asset address missing"))?;
        let vault_sdk = env.current_contract_address();
        let token = soroban_sdk::token::Client::new(env, &asset_token);
        if token.balance(&vault_sdk) != assets_i128 {
            return Err(invalid_state_error("seed custody observation mismatch"));
        }

        let now_ns = ledger_timestamp_ns(env).map_err(|_| RuntimeError::invalid_input(""))?;
        let receiver = self.register_sdk_address(env, receiver_sdk)?;
        let (effects, _) = self.apply_kernel_action_effects(
            KernelAction::SeedEpochSupply {
                receiver,
                assets_in: assets,
                now_ns: TimestampNs(now_ns),
            },
            now_ns,
        )?;
        let shares_minted = effects
            .iter()
            .find_map(|effect| match effect {
                KernelEffect::MintShares { owner, shares }
                    if *owner == receiver && *shares == assets =>
                {
                    Some(*shares)
                }
                _ => None,
            })
            .ok_or_else(|| invalid_state_error("seed mint missing"))?;
        Ok(SeedEpochSupplyResponse {
            assets_seeded: assets_i128,
            shares_minted: to_i128(shares_minted)
                .map_err(|_| RuntimeError::invalid_input(""))?,
        })
    }

    /// Take custody of a pending deposit and record the liability.
    ///
    /// The depositor must sign; custody transfers the assets into the vault
    /// contract before the ledger record exists, so a refundable liability
    /// can never be recorded against assets nobody custodied. The stored
    /// share floor travels with the record: the runtime never accepts a
    /// caller replacement.
    #[inline(never)]
    pub fn request_deposit(
        &mut self,
        env: &Env,
        owner_sdk: &SdkAddress,
        assets_i128: i128,
        min_shares_out_i128: i128,
    ) -> Result<PendingDepositResponse, RuntimeError> {
        require_signed(owner_sdk);
        self.ensure_vault_mapped(env)?;
        if !self.storage.epoch_mode_active() {
            return Err(invalid_state_error("epoch mode disabled"));
        }
        let now_ns = ledger_timestamp_ns(env).map_err(|_| RuntimeError::invalid_input(""))?;
        if self.paused {
            return Err(contract_error("paused"));
        }
        let owner = kernel_address_from_sdk(env, owner_sdk);
        let assets = to_u128(assets_i128).map_err(|_| RuntimeError::invalid_input(""))?;
        let min_shares_out =
            to_u128(min_shares_out_i128).map_err(|_| RuntimeError::invalid_input(""))?;
        let epoch = self.storage.load_epoch_state()?;
        if epoch.phase != EpochPhase::Open || !epoch.intake_epoch.is_settlement_epoch() {
            return Err(invalid_state_error("intake not open"));
        }
        // Lawful mapping on-ramp for the authenticated depositor. The
        // owner's Soroban account signed this call and is the exact source
        // of the custody transfer below, so the registered pair can only
        // ever bind the signer to its own deterministic kernel AccountId:
        // `register_sdk_address` derives the key from `owner_sdk` alone and
        // never accepts caller-supplied target bytes, so replaying the
        // registration rewrites the identical pair. The gate this replaces
        // could never pass for a first-time depositor and stranded every
        // lawful intake; it is subsumed here because every recorded
        // liability now carries a durable owner binding that admission
        // resolves after settlement for the snapshot-priced mint. Any
        // failure below aborts the whole invocation, so a request that
        // never took custody leaves no mapping, no record, and no mint.
        self.register_sdk_address(env, owner_sdk)?;
        if assets == 0 {
            return Err(RuntimeError::invalid_input("zero pending deposit"));
        }
        if self.storage.pending_deposit_stats()?.count > 0 {
            return Err(invalid_state_error("pending custody outstanding"));
        }
        // Custody first: assets must be held by the vault contract before a
        // refundable liability can be recorded against them.
        let asset_token = get_config_address(env, &VaultDataKey::AssetToken)
            .map_err(|_| RuntimeError::storage_error("asset address missing"))?;
        soroban_sdk::token::Client::new(env, &asset_token).transfer(
            owner_sdk,
            &env.current_contract_address(),
            &assets_i128,
        );
        let deposit = PendingDeposit::new(
            owner,
            assets,
            TimestampNs(now_ns),
            epoch.intake_epoch,
        )
        .map_err(|_| RuntimeError::invalid_input("pending deposit law failed"))?;
        let request_id = self
            .storage
            .create_pending_deposit(&deposit, min_shares_out)?;
        crate::effects::publish_deposit_pending(
            env,
            &owner,
            request_id,
            assets,
            epoch.intake_epoch.as_u64(),
        )?;
        Ok(PendingDepositResponse {
            request_id,
            assets: to_i128(assets).map_err(|_| RuntimeError::invalid_input(""))?,
        })
    }

    /// Cancel the caller's own pending deposit, refund the exact recorded
    /// assets once, and publish the cancellation event.
    ///
    /// The ledger removal is the at-most-once authority: once the record is
    /// gone the refund cannot be replayed, and a transaction that fails
    /// after removal rolls the refund and the removal back together.
    #[inline(never)]
    pub fn cancel_pending_deposit(
        &mut self,
        env: &Env,
        owner_sdk: &SdkAddress,
        request_id: u64,
    ) -> Result<CancelPendingDepositResponse, RuntimeError> {
        require_signed(owner_sdk);
        self.ensure_vault_mapped(env)?;
        let owner = kernel_address_from_sdk(env, owner_sdk);
        let record = self
            .storage
            .load_pending_deposit(request_id)?
            .ok_or_else(|| RuntimeError::storage_error("pending deposit not found"))?;
        if record.owner != owner {
            return Err(RuntimeError::storage_error("pending deposit not owner"));
        }
        self.storage
            .cancel_pending_deposit(&owner, request_id)?;
        let asset_token = get_config_address(env, &VaultDataKey::AssetToken)
            .map_err(|_| RuntimeError::storage_error("asset address missing"))?;
        soroban_sdk::token::Client::new(env, &asset_token).transfer(
            &env.current_contract_address(),
            owner_sdk,
            &to_i128(record.assets).map_err(|_| RuntimeError::invalid_input(""))?,
        );
        crate::effects::publish_deposit_cancelled(
            env,
            &record.owner,
            request_id,
            record.assets,
            record.epoch_id.as_u64(),
        )?;
        Ok(CancelPendingDepositResponse {
            request_id,
            assets_refunded: to_i128(record.assets).map_err(|_| RuntimeError::invalid_input(""))?,
        })
    }

    /// Close intake for the settling epoch. Allocator-authorized and
    /// Idle-only; the cutoff never prices anything.
    #[inline(never)]
    pub fn begin_epoch_cutoff(
        &mut self,
        env: &Env,
        caller_sdk: &SdkAddress,
        cutoff_ns: u64,
    ) -> Result<BeginEpochCutoffResponse, RuntimeError> {
        require_signed(caller_sdk);
        self.ensure_vault_mapped(env)?;
        if !self.storage.epoch_mode_active() {
            return Err(invalid_state_error("epoch mode disabled"));
        }
        let caller = kernel_address_from_sdk(env, caller_sdk);
        self.authorize(ActionKind::BeginEpochCutoff, caller)?;
        let now_ns = ledger_timestamp_ns(env).map_err(|_| RuntimeError::invalid_input(""))?;
        self.apply_kernel_action(
            KernelAction::BeginEpochCutoff {
                cutoff_ns: TimestampNs(cutoff_ns),
                now_ns: TimestampNs(now_ns),
            },
            now_ns,
        )?;
        let epoch = self.storage.load_epoch_state()?;
        if epoch.phase != EpochPhase::Cutoff {
            return Err(invalid_state_error("epoch cutoff not recorded"));
        }
        let cutoff = epoch
            .cutoff_ns
            .ok_or_else(|| invalid_state_error("epoch cutoff bound missing"))?;
        Ok(BeginEpochCutoffResponse {
            epoch_id: epoch.intake_epoch.as_u64(),
            cutoff_ns: cutoff.as_u64(),
        })
    }

    /// Settle the closed epoch against the complete set of authenticated
    /// adapter valuation views, read in ascending market order.
    ///
    /// Every enumerated market adapter must expose an accepted, hash-bound
    /// valuation covering the cutoff and fresh under the configured bound;
    /// a missing, erroring, or stale view fails the whole settlement closed.
    /// Values aggregate deterministically with checked arithmetic, the
    /// settlement report sequence strictly exceeds every previously accepted
    /// and settled sequence, the accepted report is recorded before dispatch,
    /// and the settlement is applied only through the kernel settlement law.
    #[inline(never)]
    pub fn settle_epoch(
        &mut self,
        env: &Env,
        caller_sdk: &SdkAddress,
    ) -> Result<SettleEpochResponse, RuntimeError> {
        require_signed(caller_sdk);
        self.ensure_vault_mapped(env)?;
        if !self.storage.epoch_mode_active() {
            return Err(invalid_state_error("epoch mode disabled"));
        }
        let caller = kernel_address_from_sdk(env, caller_sdk);
        self.authorize(ActionKind::SettleEpoch, caller)?;
        let settle_now_ns = ledger_timestamp_ns(env)
            .map_err(|_| invalid_state_error("settlement clock unavailable"))?;
        let epoch = self.storage.load_epoch_state()?;
        if epoch.phase != EpochPhase::Cutoff {
            return Err(invalid_state_error("epoch not at cutoff"));
        }
        let cutoff = epoch
            .cutoff_ns
            .ok_or_else(|| invalid_state_error("epoch cutoff bound missing"))?;
        let cutoff_ns = cutoff.as_u64();
        let max_report_age_ns = self
            .storage
            .load_max_report_age_ns()?
            .filter(|age| *age > 0)
            .ok_or_else(|| invalid_state_error("epoch freshness unconfigured"))?;
        self.storage.verify_pending_deposit_integrity()?;
        if self
            .storage
            .any_pending_deposit_before_epoch(epoch.intake_epoch)?
        {
            return Err(invalid_state_error("settlement-eligible intake outstanding"));
        }

        // The vault asset passed to each custodial adapter valuation comes
        // only from the vault's own instance storage, never a caller argument.
        let asset_address = get_config_address(env, &VaultDataKey::AssetToken)
            .map_err(|_| invalid_state_error("vault asset address missing"))?;

        let mut settlement_markets: Vec<TargetId> = Vec::new();
        let mut settlement_seq: Option<u64> = None;
        let mut settlement_as_of_ns: Option<u64> = None;
        let mut aggregate_value: u128 = 0;
        for (market, adapter) in self.storage.enumerate_market_adapter_bindings()? {
            settlement_markets.push(market);
            // An adapter error is indistinguishable from "no accepted
            // valuation" and therefore fails the settlement closed.
            let invoke_result = env.try_invoke_contract::<
                Option<CustodialValuationView>,
                soroban_sdk::Error,
            >(
                &adapter,
                &Symbol::new(env, "valuation"),
                (asset_address.clone(),).into_val(env),
            );
            let (seq, as_of_s, submitted_at, value, report_hash) = match invoke_result {
                Ok(Ok(Some(view))) => view,
                Ok(Ok(None)) => {
                    return Err(invalid_state_error("adapter valuation missing"))
                }
                Ok(Err(_)) => {
                    return Err(invalid_state_error("adapter valuation invalid"))
                }
                Err(_) => return Err(invalid_state_error("adapter valuation unavailable")),
            };
            if seq == 0 || submitted_at == 0 {
                return Err(invalid_state_error("adapter report sequence invalid"));
            }
            if report_hash.is_none() {
                return Err(invalid_state_error("adapter report hash missing"));
            }
            if as_of_s == 0 {
                return Err(invalid_state_error("adapter report valuation time zero"));
            }
            let as_of_ns = as_of_s
                .checked_mul(1_000_000_000)
                .ok_or_else(|| invalid_state_error("adapter report valuation time overflow"))?;
            if as_of_ns <= cutoff_ns {
                return Err(invalid_state_error("adapter report not after cutoff"));
            }
            if as_of_ns > settle_now_ns {
                return Err(invalid_state_error("adapter report future-dated"));
            }
            let age = settle_now_ns
                .checked_sub(as_of_ns)
                .ok_or_else(|| invalid_state_error("adapter report age underflow"))?;
            if age > max_report_age_ns {
                return Err(invalid_state_error("adapter report stale"));
            }
            match settlement_seq {
                None => settlement_seq = Some(seq),
                Some(previous) => {
                    if seq <= previous {
                        return Err(invalid_state_error(
                            "adapter report sequence not increasing",
                        ));
                    }
                }
            }
            settlement_as_of_ns = Some(match settlement_as_of_ns {
                None => as_of_ns,
                Some(earliest) => earliest.min(as_of_ns),
            });
            if value < 0 {
                return Err(invalid_state_error("adapter report value negative"));
            }
            let value_u128 = to_u128(value).map_err(|_| invalid_state_error("report value"))?;
            aggregate_value = aggregate_value
                .checked_add(value_u128)
                .ok_or_else(|| invalid_state_error("adapter valuation overflow"))?;
        }

        if settlement_seq.is_none() {
            return Err(invalid_state_error("no authenticated adapter metadata"));
        }
        let as_of_ns = settlement_as_of_ns
            .ok_or_else(|| invalid_state_error("no authenticated adapter metadata"))?;

        // The settlement NAV and eligible supply are bound from the vault's
        // own book, never from caller input.
        let settlement_nav = self.state()?.total_assets;
        let eligible_supply = self.state()?.total_shares;

        // The settlement report sequence strictly advances every previously
        // accepted and settled sequence, so a replayed or stale adapter
        // sequence can never settle.
        let last_settled_seq = epoch
            .last_settled
            .as_ref()
            .map_or(0, |snapshot| snapshot.report_seq());
        let previous_accepted = self.storage.load_accepted_report()?;
        let previous_accepted_seq =
            previous_accepted.map_or(0, |record| record.report.report_seq);
        let report_seq = last_settled_seq
            .max(previous_accepted_seq)
            .checked_add(1)
            .ok_or_else(|| invalid_state_error("settlement sequence exhausted"))?;
        if let Some(previous) = &previous_accepted {
            if report_seq <= previous.report.report_seq {
                return Err(invalid_state_error("settlement report sequence not increasing"));
            }
        }

        let report = ValuationReportRef {
            report_seq,
            as_of_ns: TimestampNs(as_of_ns),
            report_hash: Self::settlement_header_digest(
                env,
                epoch.intake_epoch.as_u64(),
                cutoff_ns,
                report_seq,
                as_of_ns,
                settlement_nav,
                eligible_supply,
            ),
        };
        self.storage
            .record_accepted_report(epoch.intake_epoch, &report)?;
        self.apply_kernel_action_effects(
            KernelAction::SettleEpoch {
                report,
                new_external_assets: aggregate_value,
                max_report_age_ns,
                settle_now_ns: TimestampNs(settle_now_ns),
            },
            settle_now_ns,
        )?;
        let settled = self.storage.load_epoch_state()?;
        let snapshot = settled
            .last_settled
            .as_ref()
            .ok_or_else(|| invalid_state_error("settled snapshot not persisted"))?;
        if snapshot.epoch_id() != epoch.intake_epoch {
            return Err(invalid_state_error("settled snapshot epoch mismatch"));
        }
        // Publish exactly one consumption record per enumerated market for
        // the consumed settlement report. The kernel already emitted the
        // single EpochSettled event during settlement.
        for market in settlement_markets.iter() {
            crate::effects::ReportMetadataConsumedEvent {
                epoch_id: snapshot.epoch_id().as_u64(),
                market_id: *market,
                report_seq: snapshot.report_seq(),
                report_hash: BytesN::from_array(env, snapshot.report_hash()),
                as_of_ns: snapshot.as_of_ns().as_u64(),
            }
            .publish(env);
        }
        Ok(SettleEpochResponse {
            epoch_id: snapshot.epoch_id().as_u64(),
            report_seq: snapshot.report_seq(),
            as_of_ns: snapshot.as_of_ns().as_u64(),
            report_hash: *snapshot.report_hash(),
            settlement_nav: to_i128(snapshot.settlement_nav())
                .map_err(|_| invalid_state_error("settlement nav"))?,
            eligible_supply: to_i128(snapshot.eligible_supply())
                .map_err(|_| invalid_state_error("eligible supply"))?,
            cutoff_ns: snapshot.cutoff_ns().as_u64(),
        })
    }

    /// Admit one recorded pending deposit against the settled epoch that
    /// covers it, using only the stored liability fields and floor.
    ///
    /// The kernel admission law is applied first; the liability record is
    /// consumed atomically with the mint afterwards, so a rejected admission
    /// can never orphan custody and an accepted one can never be replayed.
    #[inline(never)]
    pub fn admit_pending_deposit(
        &mut self,
        env: &Env,
        caller_sdk: &SdkAddress,
        request_id: u64,
    ) -> Result<AdmitPendingDepositResponse, RuntimeError> {
        require_signed(caller_sdk);
        self.ensure_vault_mapped(env)?;
        let caller = kernel_address_from_sdk(env, caller_sdk);
        self.authorize(ActionKind::AdmitPendingDeposit, caller)?;
        let record = self
            .storage
            .load_pending_deposit(request_id)?
            .ok_or_else(|| RuntimeError::storage_error("pending deposit not found"))?;
        let now_ns = ledger_timestamp_ns(env).map_err(|_| RuntimeError::invalid_input(""))?;
        // Admission resolves the recorded depositor through the durable
        // binding the intake path registered for the same authenticated
        // owner. A missing or corrupt binding fails closed before any mint
        // law is applied; a present one maps the snapshot-priced mint to
        // exactly the recorded receiver, never to caller-supplied bytes.
        self.ensure_mapped(&record.owner)?;
        let (effects, _) = self.apply_kernel_action_effects(
            KernelAction::AdmitPendingDeposit {
                receiver: record.owner,
                assets_in: record.assets,
                min_shares_out: record.min_shares_out(),
                request_epoch_id: record.epoch_id,
                now_ns: TimestampNs(now_ns),
            },
            now_ns,
        )?;
        let shares_out = effects
            .iter()
            .find_map(|effect| match effect {
                KernelEffect::MintShares { shares, .. } => Some(*shares),
                _ => None,
            })
            .ok_or_else(|| invalid_state_error("admission mint missing"))?;
        let taken = self.storage.take_pending_deposit(request_id)?;
        if taken.owner != record.owner
            || taken.assets != record.assets
            || taken.epoch_id != record.epoch_id
            || taken.min_shares_out() != record.min_shares_out()
        {
            return Err(invalid_state_error("admitted record mismatch"));
        }
        Ok(AdmitPendingDepositResponse {
            request_id,
            shares_out: to_i128(shares_out).map_err(|_| RuntimeError::invalid_input(""))?,
            assets_in: record.assets,
        })
    }

    /// Cancel the caller's own queued exit and refund its escrow.
    ///
    /// Owner binding is enforced by the kernel cancellation law; a stranger
    /// leaves the queue, the escrow, and the claim untouched.
    #[inline(never)]
    pub fn cancel_pending_withdrawal(
        &mut self,
        env: &Env,
        owner_sdk: &SdkAddress,
        request_id: u64,
    ) -> Result<CancelPendingWithdrawalResponse, RuntimeError> {
        require_signed(owner_sdk);
        self.ensure_vault_mapped(env)?;
        let caller = kernel_address_from_sdk(env, owner_sdk);
        let now_ns = ledger_timestamp_ns(env).map_err(|_| RuntimeError::invalid_input(""))?;
        let (effects, _) = self.apply_kernel_action_effects(
            KernelAction::CancelPendingWithdrawal {
                caller,
                request_id,
                now_ns: TimestampNs(now_ns),
            },
            now_ns,
        )?;
        let (escrow_shares, epoch_id) =
            effects
                .iter()
                .find_map(|effect| match effect {
                    KernelEffect::EmitEvent {
                        event:
                            templar_vault_kernel::effects::KernelEvent::WithdrawalCancelled {
                                escrow_shares,
                                epoch_id,
                                ..
                            },
                    } => Some((*escrow_shares, *epoch_id)),
                    _ => None,
                })
                .ok_or_else(|| invalid_state_error("cancellation event missing"))?;
        Ok(CancelPendingWithdrawalResponse {
            request_id,
            shares_refunded: to_i128(escrow_shares).map_err(|_| RuntimeError::invalid_input(""))?,
            epoch_id,
        })
    }

    /// Read-only epoch lifecycle view. Reports the phase code, intake epoch,
    /// cutoff, and the last settled epoch and report sequence from stored
    /// state only; it never mutates.
    #[inline(never)]
    pub fn epoch_state_view(&self) -> Result<EpochStateViewReceipt, RuntimeError> {
        let epoch = self.storage.load_epoch_state()?;
        let phase = match epoch.phase {
            EpochPhase::Open => EPOCH_PHASE_OPEN,
            EpochPhase::Cutoff => EPOCH_PHASE_CUTOFF,
            EpochPhase::Settled => EPOCH_PHASE_SETTLED,
        };
        let last_settled = epoch.last_settled.as_ref();
        Ok(EpochStateViewReceipt {
            phase,
            intake_epoch: epoch.intake_epoch.as_u64(),
            cutoff_ns: epoch.cutoff_ns.map(|cutoff| cutoff.as_u64()),
            last_settled_epoch_id: last_settled.map(|snapshot| snapshot.epoch_id().as_u64()),
            last_report_seq: last_settled.map(|snapshot| snapshot.report_seq()),
        })
    }

    /// Read-only settled-snapshot view for `epoch_id`. Reports the accepted
    /// report sequence, valuation time, report hash, settlement NAV, eligible
    /// supply, and cutoff from the immutable bound snapshot; it never mutates.
    #[inline(never)]
    pub fn epoch_snapshot_view(
        &self,
        epoch_id: u64,
    ) -> Result<EpochSnapshotReceipt, RuntimeError> {
        let epoch = self.storage.load_epoch_state()?;
        let snapshot = epoch
            .last_settled
            .as_ref()
            .filter(|snapshot| snapshot.epoch_id().as_u64() == epoch_id)
            .ok_or(RuntimeError::EpochSnapshotUnavailable)?;
        Ok(EpochSnapshotReceipt {
            epoch_id: snapshot.epoch_id().as_u64(),
            report_seq: snapshot.report_seq(),
            as_of_ns: snapshot.as_of_ns().as_u64(),
            report_hash: *snapshot.report_hash(),
            settlement_nav: to_i128(snapshot.settlement_nav())
                .map_err(|_| invalid_state_error("settlement nav"))?,
            eligible_supply: to_i128(snapshot.eligible_supply())
                .map_err(|_| invalid_state_error("eligible supply"))?,
            cutoff_ns: snapshot.cutoff_ns().as_u64(),
        })
    }

    /// Read-only per-market custodial report metadata view. Reports the
    /// latest accepted adapter valuation metadata from an authenticated
    /// adapter view, or an unavailable marker when none exists; it never
    /// mutates and never trusts caller-supplied report data.
    #[inline(never)]
    pub fn custodial_report_metadata_view(
        &self,
        env: &Env,
        market_id: TargetId,
    ) -> Result<ReportMetadataReceipt, RuntimeError> {
        let unavailable = || ReportMetadataReceipt::Unavailable { market_id };
        let adapter = match adapter_for_market(env, market_id) {
            Ok(adapter) => adapter,
            Err(_) => return Ok(unavailable()),
        };
        let asset_address = match get_config_address(env, &VaultDataKey::AssetToken) {
            Ok(asset_address) => asset_address,
            Err(_) => return Ok(unavailable()),
        };
        let view = match env.try_invoke_contract::<
            Option<CustodialValuationView>,
            soroban_sdk::Error,
        >(
            &adapter,
            &Symbol::new(env, "valuation"),
            (asset_address,).into_val(env),
        ) {
            Ok(Ok(Some(view))) => view,
            _ => return Ok(unavailable()),
        };
        let (seq, as_of, submitted_at, assets_value, report_hash) = view;
        Ok(ReportMetadataReceipt::Available {
            market_id,
            seq,
            as_of,
            submitted_at,
            assets_value,
            report_hash: report_hash.map(|hash| hash.to_array()),
        })
    }

    /// Domain-bound settlement header digest. The vault binds the exact
    /// values it is settling for this epoch, cutoff, and its own contract
    /// identity, so a settlement cannot cite a report that was never bound
    /// to this vault.
    fn settlement_header_digest(
        env: &Env,
        epoch_id: u64,
        cutoff_ns: u64,
        report_seq: u64,
        as_of_ns: u64,
        settlement_nav: u128,
        eligible_supply: u128,
    ) -> [u8; 32] {
        const SETTLEMENT_HEADER_DOMAIN: &[u8] = b"templar:soroban:settlement:v1";
        let mut preimage = soroban_sdk::Bytes::new(env);
        preimage.extend_from_slice(SETTLEMENT_HEADER_DOMAIN);
        preimage.extend_from_slice(&epoch_id.to_be_bytes());
        preimage.extend_from_slice(&cutoff_ns.to_be_bytes());
        preimage.extend_from_slice(&report_seq.to_be_bytes());
        preimage.extend_from_slice(&as_of_ns.to_be_bytes());
        preimage.extend_from_slice(&settlement_nav.to_be_bytes());
        preimage.extend_from_slice(&eligible_supply.to_be_bytes());
        preimage.extend_from_slice(
            kernel_address_from_sdk(env, &env.current_contract_address()).as_bytes(),
        );
        env.crypto().sha256(&preimage).to_bytes().to_array()
    }
}
