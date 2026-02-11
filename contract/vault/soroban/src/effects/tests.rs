use super::*;
use soroban_sdk::testutils::Address as _;

#[derive(Clone, Debug, Default)]
struct TestSep41Token {
    should_fail: bool,
    mock_balance: i128,
}

impl TestSep41Token {
    fn new() -> Self {
        Self {
            should_fail: false,
            mock_balance: 1000,
        }
    }

    fn failing() -> Self {
        Self {
            should_fail: true,
            mock_balance: 0,
        }
    }
}

impl Sep41Token for TestSep41Token {
    fn mint(&self, _to: &Address, _amount: i128) -> EffectResult<()> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("test mint failed"));
        }
        Ok(())
    }

    fn burn(&self, _from: &Address, _amount: i128) -> EffectResult<()> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("test burn failed"));
        }
        Ok(())
    }

    fn transfer(&self, _from: &Address, _to: &Address, _amount: i128) -> EffectResult<()> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("test transfer failed"));
        }
        Ok(())
    }

    fn balance(&self, _addr: &Address) -> EffectResult<i128> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("test balance failed"));
        }
        Ok(self.mock_balance)
    }
}

fn test_env() -> Env {
    Env::default()
}

fn test_context() -> EffectContext {
    EffectContext::new(1_000_000_000_000, [1u8; 32], [2u8; 32], [3u8; 32])
}

#[test]
fn test_effect_summary_new() {
    let summary = EffectSummary::new();
    assert_eq!(summary.shares_minted, 0);
    assert_eq!(summary.shares_burned, 0);
    assert_eq!(summary.shares_transferred, 0);
    assert_eq!(summary.assets_transferred, 0);
    assert_eq!(summary.events_emitted, 0);
}

#[test]
fn test_effect_summary_recording() {
    let mut summary = EffectSummary::new();

    summary.record_mint(100);
    assert_eq!(summary.shares_minted, 100);

    summary.record_burn(50);
    assert_eq!(summary.shares_burned, 50);

    summary.record_share_transfer(25);
    assert_eq!(summary.shares_transferred, 25);

    summary.record_asset_transfer(1000);
    assert_eq!(summary.assets_transferred, 1000);

    summary.record_event();
    summary.record_event();
    assert_eq!(summary.events_emitted, 2);
}

#[test]
fn test_effect_context_new() {
    let ctx = test_context();
    assert_eq!(ctx.now_ns, 1_000_000_000_000);
    assert_eq!(ctx.vault_address, [1u8; 32]);
    assert_eq!(ctx.asset_address, [2u8; 32]);
    assert_eq!(ctx.share_address, [3u8; 32]);
}

#[test]
fn test_test_sep41_token_mint() {
    let env = test_env();
    let token = TestSep41Token::new();
    let addr = Address::generate(&env);
    let result = token.mint(&addr, 100);
    assert!(result.is_ok());
}

#[test]
fn test_test_sep41_token_burn() {
    let env = test_env();
    let token = TestSep41Token::new();
    let addr = Address::generate(&env);
    let result = token.burn(&addr, 50);
    assert!(result.is_ok());
}

#[test]
fn test_test_sep41_token_transfer() {
    let env = test_env();
    let token = TestSep41Token::new();
    let from = Address::generate(&env);
    let to = Address::generate(&env);
    let result = token.transfer(&from, &to, 25);
    assert!(result.is_ok());
}

#[test]
fn test_test_sep41_token_balance() {
    let env = test_env();
    let token = TestSep41Token::new();
    let addr = Address::generate(&env);
    let result = token.balance(&addr);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 1000);
}

#[test]
fn test_test_sep41_token_failing() {
    let env = test_env();
    let token = TestSep41Token::failing();
    let addr = Address::generate(&env);
    let from = Address::generate(&env);
    let to = Address::generate(&env);

    assert!(token.mint(&addr, 100).is_err());
    assert!(token.burn(&addr, 50).is_err());
    assert!(token.transfer(&from, &to, 25).is_err());
    assert!(token.balance(&addr).is_err());
}

#[test]
fn test_u128_to_i128_conversion() {
    // Valid conversions
    assert!(to_i128_event(0).is_ok());
    assert!(to_i128_event(1000).is_ok());
    assert!(to_i128_event(i128::MAX as u128).is_ok());

    // Overflow
    assert!(to_i128_event((i128::MAX as u128) + 1).is_err());
}

#[test]
fn test_address_map() {
    let env = test_env();
    let mut map = AddressMap::new(&env);

    let kernel_addr = [1u8; 32];
    let soroban_addr = Address::generate(&env);

    map.register(kernel_addr, soroban_addr.clone());

    let resolved = map.resolve(&kernel_addr);
    assert!(resolved.is_some());
    assert_eq!(resolved.unwrap(), &soroban_addr);

    // Unknown address
    let unknown = [2u8; 32];
    assert!(map.resolve(&unknown).is_none());
}

#[test]
fn test_emit_event_serializes_without_address_mapping() {
    use templar_vault_kernel::effects::KernelEvent;

    let env = test_env();
    let share = TestSep41Token::new();
    let asset = TestSep41Token::new();
    let mut interpreter = SorobanEffectInterpreter::new(&env, &share, &asset);
    let ctx = test_context();

    let effect = KernelEffect::EmitEvent {
        event: KernelEvent::DepositProcessed {
            owner: [1u8; 32],
            receiver: [2u8; 32],
            assets_in: 1,
            shares_out: 1,
        },
    };

    assert!(interpreter.execute_effect(&effect, &ctx).is_ok());
}
