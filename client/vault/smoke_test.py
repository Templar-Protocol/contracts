#!/usr/bin/env python3
"""Smoke test for templar_vault_client Python bindings."""

import asyncio
import os
import sys
from dataclasses import dataclass
from typing import Optional

# Import the generated bindings
from templar_vault_client import (
    KeyPoolClient,
    KeyPoolConfig,
    KeyCredential,
    VaultClient,
    VaultViewClient,
    VaultClientConfig,
    AccountId,
    ErrorWrapper,
    AllocationDelta,
    Delta,
    StorageBalance,
    StorageBalanceBounds,
)

DEFAULT_RPC_URL = "https://rpc.testnet.fastnear.com"
LOOP_TIMEOUT_SECONDS = int(os.environ.get("SMOKE_LOOP_TIMEOUT_SECONDS", "120"))
DUMMY_SECRET_KEY = os.environ.get("DUMMY_SECRET_KEY")


# =============================================================================
# Configuration for Integration Tests
# =============================================================================


@dataclass
class SmokeTestConfig:
    """Configuration for integration tests against a deployed vault."""

    rpc_url: str
    vault_account: str
    rpc_api_key: Optional[str] = None
    underlying_token: Optional[str] = None
    market_account: Optional[str] = None
    # Credentials for different roles
    user_account: Optional[str] = None
    user_secret_key: Optional[str] = None
    curator_account: Optional[str] = None
    curator_secret_key: Optional[str] = None
    allocator_account: Optional[str] = None
    allocator_secret_key: Optional[str] = None

    @classmethod
    def from_env(cls) -> "SmokeTestConfig":
        """Load configuration from environment variables.

        Raises:
            ValueError: If VAULT_ACCOUNT is missing.
        """
        vault_account = os.environ.get("VAULT_ACCOUNT")
        if not vault_account:
            raise ValueError(
                "VAULT_ACCOUNT environment variable is required for "
                "SmokeTestConfig.from_env"
            )

        return cls(
            rpc_url=os.environ.get("RPC_URL", DEFAULT_RPC_URL),
            vault_account=vault_account,
            rpc_api_key=os.environ.get("RPC_API_KEY"),
            underlying_token=os.environ.get("UNDERLYING_TOKEN"),
            market_account=os.environ.get("MARKET_ACCOUNT"),
            user_account=os.environ.get("USER_ACCOUNT"),
            user_secret_key=os.environ.get("USER_SECRET_KEY"),
            curator_account=os.environ.get("CURATOR_ACCOUNT"),
            curator_secret_key=os.environ.get("CURATOR_SECRET_KEY"),
            allocator_account=os.environ.get("ALLOCATOR_ACCOUNT"),
            allocator_secret_key=os.environ.get("ALLOCATOR_SECRET_KEY"),
        )


def dummy_credential(account_id: str = "dummy.testnet") -> Optional[KeyCredential]:
    """Build a dummy credential if DUMMY_SECRET_KEY is configured."""
    if not DUMMY_SECRET_KEY:
        return None
    return KeyCredential(
        account_id=AccountId(account_id),
        secret_key=DUMMY_SECRET_KEY,
    )


def test_imports():
    """Test that all expected classes are importable."""
    print("✓ All imports successful")


def test_key_credential_creation():
    """Test KeyCredential dataclass creation."""
    cred = KeyCredential(
        account_id=AccountId("test.near"),
        secret_key="not-a-real-key",
    )
    assert cred.account_id == "test.near"
    print("✓ KeyCredential creation works")


def test_key_pool_config_defaults():
    """Test KeyPoolConfig with default-ish values."""
    config = KeyPoolConfig(
        timeout_seconds=60,
        retry=None,
        max_nonce_retries=3,
        block_hash_ttl_seconds=30,
        view_cache_capacity=100,
        view_cache_ttl_seconds=5,
        rpc_api_key=None,
    )
    assert config.timeout_seconds == 60
    assert config.max_nonce_retries == 3
    print("✓ KeyPoolConfig creation works")


def test_vault_client_config():
    """Test VaultClientConfig creation."""
    config = VaultClientConfig(
        timeout_seconds=60,
        retry=None,
        max_nonce_retries=3,
        block_hash_ttl_seconds=30,
        view_cache_capacity=100,
        view_cache_ttl_seconds=5,
        rpc_api_key=None,
    )
    assert config.timeout_seconds == 60
    print("✓ VaultClientConfig creation works")


def test_invalid_credential_rejected():
    """Test that invalid credentials are rejected at client creation."""
    config = KeyPoolConfig(
        timeout_seconds=60,
        retry=None,
        max_nonce_retries=3,
        block_hash_ttl_seconds=30,
        view_cache_capacity=100,
        view_cache_ttl_seconds=5,
        rpc_api_key=None,
    )

    # Invalid secret key
    cred = KeyCredential(
        account_id=AccountId("test.near"),
        secret_key="not-a-valid-key",
    )

    try:
        KeyPoolClient(
            rpc_url="https://rpc.testnet.fastnear.com",
            vault=AccountId("vault.testnet"),
            credentials=[cred],
            config=config,
        )
        print("✗ Expected error for invalid key")
        sys.exit(1)
    except ErrorWrapper:
        print("✓ Invalid credential correctly rejected")


def test_empty_credentials_rejected():
    """Test that empty credentials list is rejected."""
    config = KeyPoolConfig(
        timeout_seconds=60,
        retry=None,
        max_nonce_retries=3,
        block_hash_ttl_seconds=30,
        view_cache_capacity=100,
        view_cache_ttl_seconds=5,
        rpc_api_key=None,
    )

    try:
        KeyPoolClient(
            rpc_url="https://rpc.testnet.fastnear.com",
            vault=AccountId("vault.testnet"),
            credentials=[],
            config=config,
        )
        print("✗ Expected error for empty credentials")
        sys.exit(1)
    except ErrorWrapper:
        print("✓ Empty credentials correctly rejected")


async def test_client_creation_with_valid_key():
    """Test client creation with a valid (but non-funded) key."""
    cred = dummy_credential("test.testnet")
    if cred is None:
        print("⚠ Skipping valid-key creation test (set DUMMY_SECRET_KEY)")
        return

    config = KeyPoolConfig(
        timeout_seconds=10,
        retry=None,
        max_nonce_retries=3,
        block_hash_ttl_seconds=30,
        view_cache_capacity=100,
        view_cache_ttl_seconds=5,
        rpc_api_key=None,
    )

    client = KeyPoolClient(
        rpc_url="https://rpc.testnet.fastnear.com",
        vault=AccountId("vault.testnet"),
        credentials=[cred],
        config=config,
    )

    # Check pool health
    health = client.get_pool_health()
    assert health.total_keys == 1
    assert health.healthy_keys == 1
    print(
        f"✓ Client created successfully (pool: {health.healthy_keys}/{health.total_keys} healthy)"
    )

    # Test vault_account getter
    vault = client.vault_account()
    assert vault == "vault.testnet"
    print(f"✓ vault_account() returns: {vault}")


async def test_vault_client_creation():
    """Test VaultClient (the new unified client) creation."""
    cred = dummy_credential("test.testnet")
    if cred is None:
        print("⚠ Skipping VaultClient creation test (set DUMMY_SECRET_KEY)")
        return

    config = VaultClientConfig(
        timeout_seconds=10,
        retry=None,
        max_nonce_retries=3,
        block_hash_ttl_seconds=30,
        view_cache_capacity=100,
        view_cache_ttl_seconds=5,
        rpc_api_key=None,
    )

    client = VaultClient.new_key_pool(
        rpc_url="https://rpc.testnet.fastnear.com",
        vault=AccountId("vault.testnet"),
        credentials=[cred],
        config=config,
    )

    health = client.get_pool_health()
    assert health.total_keys == 1
    print(
        f"✓ VaultClient.new_key_pool() works (pool: {health.healthy_keys}/{health.total_keys})"
    )


async def test_vault_client_single_key_default():
    """Test VaultClient.new_single_key_default() - the simple single-key API."""
    cred = dummy_credential("test.testnet")
    if cred is None:
        print("⚠ Skipping single-key default test (set DUMMY_SECRET_KEY)")
        return

    client = VaultClient.new_single_key_default(
        rpc_url="https://rpc.testnet.fastnear.com",
        vault=AccountId("vault.testnet"),
        credential=cred,
    )

    health = client.get_pool_health()
    assert health.total_keys == 1
    assert health.healthy_keys == 1

    vault = client.vault_account()
    assert vault == "vault.testnet"

    print(f"✓ VaultClient.new_single_key_default() works (vault: {vault})")


# =============================================================================
# Integration Tests (require deployed vault)
# =============================================================================


async def test_view_methods(config: SmokeTestConfig):
    """Test all view methods return valid data against a live vault.

    This is a read-only test that doesn't require funded accounts or signing.
    It uses `VaultViewClient`, which does not require key material.
    """
    # Use custom config if API key is provided.
    client_config = KeyPoolConfig(
        timeout_seconds=60,
        retry=None,
        max_nonce_retries=3,
        block_hash_ttl_seconds=30,
        view_cache_capacity=100,
        view_cache_ttl_seconds=5,
        rpc_api_key=config.rpc_api_key,
    )

    client = VaultViewClient.new(
        rpc_url=config.rpc_url,
        vault=AccountId(config.vault_account),
        config=client_config,
    )

    print(f"Testing view methods against vault: {config.vault_account}")
    print(f"  RPC URL: {config.rpc_url}")
    print(f"  API Key: {'configured' if config.rpc_api_key else 'not set'}")
    print()

    # === Basic U128 getters ===
    total_assets = await client.get_total_assets()
    print(f"  Total assets: {total_assets}")

    total_supply = await client.get_total_supply()
    print(f"  Total supply: {total_supply}")

    idle_balance = await client.get_idle_balance()
    print(f"  Idle balance: {idle_balance}")

    max_deposit = await client.get_max_deposit()
    print(f"  Max deposit: {max_deposit}")

    max_single = await client.get_max_single_market_deposit()
    print(f"  Max single market deposit: {max_single}")

    last_total = await client.get_last_total_assets()
    print(f"  Last total assets: {last_total}")

    print("✓ Basic U128 getters work")

    # === Configuration ===
    vault_config = await client.get_configuration()
    print(
        f"  Configuration: owner={vault_config.owner}, curator={vault_config.curator}"
    )

    fees = await client.get_fees()
    print(f"  Fees: management={fees.management}, performance={fees.performance}")

    restrictions = await client.get_restrictions()
    print(f"  Restrictions: {restrictions}")

    fee_anchor = await client.get_fee_anchor()
    print(
        f"  Fee anchor: timestamp_ns={fee_anchor.timestamp_ns}, total_assets={fee_anchor.total_assets}"
    )

    print("✓ Configuration getters work")

    # === Markets ===
    markets = await client.list_markets_with_ids()
    print(f"  Markets registered: {len(markets)}")
    for m in markets:
        print(f"    - ID {m.market_id}: {m.account}")

    cap_groups = await client.get_cap_groups()
    print(f"  Cap groups: {len(cap_groups)}")

    print("✓ Market getters work")

    # === Withdrawal state ===
    withdrawing_op = await client.get_withdrawing_op_id()
    print(f"  Withdrawing op ID: {withdrawing_op}")

    pending_id = await client.peek_next_pending_withdrawal_id()
    print(f"  Queue head (next to process): {pending_id}")

    current_req = await client.get_current_withdraw_request_id()
    print(f"  Current withdraw request ID: {current_req}")

    queue_tail = await client.queue_tail()
    print(f"  Queue tail (next ID to assign): {queue_tail}")

    has_pending = await client.has_pending_market_withdrawal()
    print(f"  Has pending market withdrawal: {has_pending}")

    print("✓ Withdrawal state getters work")

    # === Pending governance ===
    pending_gov = await client.get_pending_governance_actions()
    print(f"  Pending governance actions: {len(pending_gov)}")

    print("✓ Governance getters work")

    # === Comprehensive snapshot ===
    snapshot = await client.get_vault_snapshot()
    print(f"  Snapshot: assets={snapshot.total_assets}, supply={snapshot.total_supply}")

    print("✓ Vault snapshot works")

    # === Conversion previews ===
    if int(total_assets) > 0:
        # Only test previews if vault has assets
        preview_dep = await client.preview_deposit("1000000")
        print(f"  Preview deposit 1000000 -> {preview_dep} shares")

        preview_red = await client.preview_redeem("1000000")
        print(f"  Preview redeem 1000000 shares -> {preview_red} assets")

        print("✓ Preview methods work")
    else:
        print("  (Skipping preview methods - vault has no assets)")

    print()
    print("✓ All view methods passed!")


async def test_happy_path_flow(config: SmokeTestConfig):
    """
    Test vault operations flow:
    1. Check initial vault state
    2. Deposit tokens via ft_transfer_call (if UNDERLYING_TOKEN set)
    3. Reallocate to market (if allocator available and idle > 0)
    4. Request withdrawal (redeem shares)
    5. Execute withdrawal route (allocator)
    6. Verify final state

    Requires: USER_ACCOUNT, USER_SECRET_KEY
    Optional: UNDERLYING_TOKEN (for deposit test)
    Optional: ALLOCATOR_ACCOUNT, ALLOCATOR_SECRET_KEY (for reallocate/execute)
    """
    # Skip if no user credentials
    if not config.user_account or not config.user_secret_key:
        print("⚠ Skipping happy path: USER_ACCOUNT/USER_SECRET_KEY not set")
        return

    print(f"Running happy path flow against vault: {config.vault_account}")
    print(f"  User account: {config.user_account}")
    print(f"  RPC URL: {config.rpc_url}")
    print(f"  API Key: {'configured' if config.rpc_api_key else 'not set'}")
    print()

    # Client config with optional API key
    client_config = VaultClientConfig(
        timeout_seconds=60,
        retry=None,
        max_nonce_retries=3,
        block_hash_ttl_seconds=30,
        view_cache_capacity=100,
        view_cache_ttl_seconds=5,
        rpc_api_key=config.rpc_api_key,
    )

    # === Setup user client ===
    user_cred = KeyCredential(
        account_id=AccountId(config.user_account),
        secret_key=config.user_secret_key,
    )

    user_client = VaultClient.new_single_key(
        rpc_url=config.rpc_url,
        vault=AccountId(config.vault_account),
        credential=user_cred,
        config=client_config,
    )

    # === Setup allocator client (if available) ===
    allocator_client = None
    print(f"  Allocator account from env: {config.allocator_account or '(not set)'}")
    if config.allocator_account and config.allocator_secret_key:
        allocator_cred = KeyCredential(
            account_id=AccountId(config.allocator_account),
            secret_key=config.allocator_secret_key,
        )
        allocator_client = VaultClient.new_single_key(
            rpc_url=config.rpc_url,
            vault=AccountId(config.vault_account),
            credential=allocator_cred,
            config=client_config,
        )
        print(f"  ✓ Allocator client created for: {config.allocator_account}")
    else:
        print("  ⚠ Allocator client NOT created (missing credentials)")

    # === Step 0: Clear any in-progress operations ===
    withdrawing_op = await user_client.get_withdrawing_op_id()
    if withdrawing_op is not None and allocator_client:
        print(f"Step 0 - Clearing in-progress withdrawal (op_id={withdrawing_op})...")
        markets_for_clear = await user_client.list_markets_with_ids()
        try:
            max_iterations = 10
            for _i in range(max_iterations):
                op_id = await user_client.get_withdrawing_op_id()
                if op_id is None:
                    print("  ✓ Vault now idle")
                    break

                for market in markets_for_clear:
                    try:
                        await allocator_client.execute_market_withdrawal(
                            op_id,
                            market.market_id,
                            None,
                        )
                        print(f"  Executed withdrawal from market {market.market_id}")
                    except ErrorWrapper as e:
                        print(
                            f"  ⚠ execute_market_withdrawal failed for market {market.market_id}: {e}"
                        )
            else:
                print(
                    f"  ⚠ Could not clear withdrawal after {max_iterations} iterations"
                )
        except ErrorWrapper as e:
            print(f"  ⚠ Error clearing withdrawal: {e}")
    elif withdrawing_op is not None:
        print(
            f"⚠ Withdrawal in progress (op_id={withdrawing_op}) but no allocator to clear it"
        )

    # === Step 1: Check initial state ===
    initial_assets = await user_client.get_total_assets()
    initial_supply = await user_client.get_total_supply()
    initial_idle = await user_client.get_idle_balance()
    max_deposit = await user_client.get_max_deposit()
    markets = await user_client.list_markets_with_ids()
    print("Step 1 - Initial state:")
    print(f"  Total assets: {initial_assets}")
    print(f"  Total supply: {initial_supply}")
    print(f"  Idle balance: {initial_idle}")
    print(f"  Max deposit: {max_deposit}")
    print(f"  Markets registered: {len(markets)}")
    for m in markets:
        print(f"    - ID {m.market_id}: {m.account}")

    # === Step 1.5: Curator setup (if allocator available) ===
    if allocator_client:
        print()
        print(
            "Step 1.5 - Curator setup (registering vault and setting supply_queue)..."
        )

        vault_account = AccountId(config.vault_account)
        storage_deposit_min_yocto = (
            "1250000000000000000000"  # 0.00125 NEAR for tokens
        )
        market_storage_min_yocto = (
            "10000000000000000000000"  # 0.01 NEAR for markets
        )

        # Register vault with itself (NEP-145)
        # This is needed for the vault to hold its own shares (fee accrual)
        print("  Checking vault self-registration...")
        try:
            vault_self_balance = await allocator_client.storage_balance_of(
                vault_account
            )
            if vault_self_balance is None:
                print("    Registering vault with itself...")
                bounds = await allocator_client.storage_balance_bounds()
                storage = await allocator_client.storage_deposit(
                    vault_account,
                    bounds.min,
                )
                print(f"    ✓ Vault registered with itself: total={storage.total}")
            else:
                print(f"    ✓ Already registered: total={vault_self_balance.total}")
        except ErrorWrapper as e:
            print(f"    ⚠ Vault self-registration: {e}")

        # Register vault with underlying token contract (NEP-145)
        # This is needed so the vault can receive tokens from depositors
        if config.underlying_token:
            print(
                f"  Checking vault registration with token {config.underlying_token}..."
            )
            try:
                token_balance = await allocator_client.token_storage_balance_of(
                    AccountId(config.underlying_token),
                    vault_account,
                )
                if token_balance is None:
                    print("    Registering vault with token...")
                    storage = await allocator_client.token_storage_deposit(
                        AccountId(config.underlying_token),
                        vault_account,
                        storage_deposit_min_yocto,
                    )
                    print(f"    ✓ Vault registered with token: total={storage.total}")
                else:
                    print(f"    ✓ Already registered: total={token_balance.total}")
            except ErrorWrapper as e:
                print(f"    ⚠ Token storage registration: {e}")

        if not markets and config.market_account:
            print(f"  No markets registered - adding market {config.market_account}...")
            market_cap = "1000000000000000000000000"  # 1M tokens (assuming 18 decimals)

            # Submit cap for the market
            print("    Submitting cap for market...")
            try:
                await allocator_client.submit_cap(
                    AccountId(config.market_account),
                    market_cap,
                )
                print("    ✓ submit_cap succeeded")
            except ErrorWrapper as e:
                print(f"    ⚠ submit_cap failed: {e}")

            # Accept cap (may fail if timelock > 0)
            print("    Accepting cap for market...")
            try:
                await allocator_client.accept_cap(
                    AccountId(config.market_account),
                )
                print("    ✓ accept_cap succeeded")

                # Refresh markets list
                markets = await user_client.list_markets_with_ids()
                print(f"    Markets after setup: {len(markets)}")
            except ErrorWrapper as e:
                print(f"    ⚠ accept_cap failed: {e}")
                print("      (May need to wait for timelock, or already accepted)")

        if not markets:
            print("  ⚠ No markets registered - cannot set supply_queue")
            if not config.market_account:
                print("    Set MARKET_ACCOUNT env var to add a market")
        else:
            # Register vault with each market contract (NEP-145)
            # This is needed so the vault can supply tokens to markets
            for market in markets:
                print(f"  Checking vault registration with market {market.account}...")
                try:
                    market_balance = await allocator_client.token_storage_balance_of(
                        AccountId(market.account),
                        vault_account,
                    )
                    if market_balance is None:
                        print("    Registering vault with market...")
                        storage = await allocator_client.token_storage_deposit(
                            AccountId(market.account),
                            vault_account,
                            market_storage_min_yocto,
                        )
                        print(
                            f"    ✓ Vault registered with market: total={storage.total}"
                        )
                    else:
                        print(f"    ✓ Already registered: total={market_balance.total}")
                except ErrorWrapper as e:
                    print(f"    ⚠ Market storage registration: {e}")

            # Set supply queue with all markets (if max_deposit is 0)
            if int(max_deposit) == 0:
                print(f"  Setting supply_queue with {len(markets)} markets...")
                try:
                    market_ids = [m.market_id for m in markets]
                    supply_queue_deposit = (
                        "840000000000000000000"  # ~0.00084 NEAR per market
                    )
                    await allocator_client.set_supply_queue(
                        markets=market_ids,
                        deposit_yocto=supply_queue_deposit,
                    )
                    print("  ✓ Supply queue set")

                    # Re-check max_deposit
                    max_deposit = await user_client.get_max_deposit()
                    print(f"  Max deposit after setup: {max_deposit}")
                except ErrorWrapper as e:
                    print(f"  ⚠ set_supply_queue failed: {e}")
            else:
                print(f"  Supply queue already configured (max_deposit={max_deposit})")

    if not allocator_client and int(max_deposit) == 0:
        print()
        print("⚠ WARNING: max_deposit is 0 - deposits will be refunded!")
        print("  This means either:")
        print("    1. No markets are in the supply_queue, OR")
        print("    2. All markets are at capacity (cap full)")
        print("  Set ALLOCATOR_ACCOUNT/ALLOCATOR_SECRET_KEY to enable curator setup.")

    # === Step 2: Deposit tokens ===
    if config.underlying_token:
        deposit_amount = "1000000"  # 1 token (assuming 6 decimals like USDT)
        print(f"Step 2 - Depositing {deposit_amount} tokens via ft_transfer_call...")
        print(f"  Token: {config.underlying_token}")

        # Pre-deposit sanity check: token balance can persist even if the vault account was deleted
        # and redeployed with the same account ID. In that case, the vault's stored idle_balance
        # will be stale (0) until refresh_idle_balance.
        try:
            vault_id = AccountId(config.vault_account)
            token_balance = await user_client.ft_balance_of(
                token=AccountId(config.underlying_token),
                account_id=vault_id,
            )
            idle_before = await user_client.get_idle_balance()
            print(f"  Vault token balance (ft_balance_of): {token_balance}")
            print(f"  Vault idle_balance (accounted):     {idle_before}")
            if int(token_balance) > int(idle_before):
                print(
                    "  ⚠ Token balance > idle_balance. If this vault was redeployed, you likely need refresh_idle_balance before deposits."
                )
        except ErrorWrapper as e:
            print(f"  ⚠ Pre-deposit token balance check failed: {e}")

        # Register user with vault (NEP-145) if not already registered
        print("  Checking vault storage registration...")
        try:
            vault_storage = await user_client.storage_balance_of(
                account_id=AccountId(config.user_account)
            )
            if vault_storage is None:
                print("  Registering with vault...")
                bounds = await user_client.storage_balance_bounds()
                print(f"    Storage bounds: min={bounds.min}, max={bounds.max}")
                vault_storage = await user_client.storage_deposit(
                    account_id=None,  # sender
                    deposit_yocto=bounds.min,
                )
                print(f"  ✓ Vault storage registered: total={vault_storage.total}")
            else:
                print(f"  ✓ Already registered with vault: total={vault_storage.total}")
        except ErrorWrapper as e:
            print(f"  ⚠ Vault storage registration failed: {e}")

        # Register user with token contract (NEP-145) if not already registered
        print(f"  Checking token storage registration for {config.underlying_token}...")
        try:
            # Note: We register ourselves with the token so we can send tokens
            # Typical minimum storage deposit for NEP-141 tokens is 0.00125 NEAR
            storage_deposit_yocto = "1250000000000000000000"  # 0.00125 NEAR
            token_storage = await user_client.token_storage_deposit(
                token=AccountId(config.underlying_token),
                account_id=None,  # sender
                deposit_yocto=storage_deposit_yocto,
            )
            print(f"  ✓ Token storage registered: total={token_storage.total}")
        except ErrorWrapper as e:
            # This might fail if already registered, which is fine
            print(f"  ⚠ Token storage registration: {e}")
            print("    (May already be registered, continuing...)")

        try:
            used = await user_client.ft_transfer_call(
                token=AccountId(config.underlying_token),
                amount=deposit_amount,
                msg=None,
            )
            print(f"✓ Deposit transaction submitted (used by vault: {used})")

            await user_client.clear_view_cache()
            if allocator_client:
                await allocator_client.clear_view_cache()

            # Verify assets increased
            after_deposit_assets = await user_client.get_total_assets()
            after_deposit_idle = await user_client.get_idle_balance()
            print(f"  Total assets after: {after_deposit_assets}")
            print(f"  Idle balance after: {after_deposit_idle}")

            if int(max_deposit) == 0:
                if int(used) == 0 and int(after_deposit_idle) == int(initial_idle):
                    print("✓ Deposit refunded (expected: max_deposit=0)")
                else:
                    print(
                        "⚠ Deposit behavior unexpected: max_deposit=0 but deposit may have partially succeeded"
                    )
            else:
                if int(after_deposit_idle) > int(initial_idle):
                    print("✓ Deposit verified: idle balance increased")
                else:
                    print("⚠ Deposit: idle balance did not increase")

            # Donation demo: plain ft_transfer (no receiver hook) + refresh_idle_balance.
            # This is the intended "donation" behavior: token balance changes immediately,
            # but vault idle_balance only changes after refresh_idle_balance.
            if allocator_client:
                try:
                    donation_amount = "1"
                    vault_id = AccountId(config.vault_account)

                    print(
                        f"Step 2b - Donating {donation_amount} token via ft_transfer (no hook)..."
                    )

                    before_token = await allocator_client.ft_balance_of(
                        token=AccountId(config.underlying_token),
                        account_id=vault_id,
                    )
                    before_idle = await user_client.get_idle_balance()

                    await allocator_client.ft_transfer(
                        token=AccountId(config.underlying_token),
                        amount=donation_amount,
                        memo="smoke_test donation",
                    )

                    await user_client.clear_view_cache()
                    await allocator_client.clear_view_cache()

                    after_token = await allocator_client.ft_balance_of(
                        token=AccountId(config.underlying_token),
                        account_id=vault_id,
                    )
                    after_idle_before_refresh = await user_client.get_idle_balance()

                    print(f"  token balance before: {before_token}")
                    print(f"  token balance after:  {after_token}")
                    print(f"  idle before donation: {before_idle}")
                    print(f"  idle before refresh:  {after_idle_before_refresh}")

                    print("  Calling refresh_idle_balance...")
                    report = await allocator_client.refresh_idle_balance()

                    await user_client.clear_view_cache()
                    await allocator_client.clear_view_cache()

                    after_idle = await user_client.get_idle_balance()

                    print(f"  refresh_idle_balance outcome: {report.outcome}")
                    print(f"  idle after refresh:  {after_idle}")
                except ErrorWrapper as e:
                    print(f"  ⚠ donation/refresh smoke step failed: {e}")
        except ErrorWrapper as e:
            print(f"⚠ Deposit failed: {e}")
            print("  (User may not have sufficient token balance or allowance)")
            after_deposit_idle = initial_idle
    else:
        print("Step 2 - Skipping deposit (UNDERLYING_TOKEN not set)")
        print("  Set UNDERLYING_TOKEN env var to test deposits")
        after_deposit_idle = initial_idle

    # === Step 3: Reallocate to market (if allocator available and idle > 0) ===
    if allocator_client and markets and int(after_deposit_idle) > 0:
        market = markets[0]
        # Reallocate a small amount from idle to market
        reallocate_amount = str(min(int(after_deposit_idle), 1000000))  # Up to 1 token
        print(
            f"Step 3 - Reallocating {reallocate_amount} to market {market.market_id} ({market.account})..."
        )

        delta = AllocationDelta.SUPPLY(
            Delta(market=market.market_id, amount=reallocate_amount)
        )
        try:
            await allocator_client.reallocate(delta)
            print("✓ Reallocate transaction submitted")

            # Verify idle decreased
            new_idle = await user_client.get_idle_balance()
            print(f"  New idle balance: {new_idle}")

            if int(new_idle) < int(after_deposit_idle):
                print("✓ Reallocate verified: idle balance decreased")
            else:
                print("⚠ Reallocate: idle balance did not decrease (may need harvest)")
        except ErrorWrapper as e:
            print(f"⚠ Reallocate failed: {e}")
    else:
        if not allocator_client:
            print("Step 3 - Skipping reallocate: no allocator credentials")
        elif not markets:
            print("Step 3 - Skipping reallocate: no markets registered")
        else:
            print("Step 3 - Skipping reallocate: no idle balance to reallocate")

    # === Step 4: Request withdrawal (redeem shares) ===
    # Only attempt redeem if shares exist.
    try:
        await user_client.clear_view_cache()
    except ErrorWrapper as e:
        print(f"  ⚠ clear_view_cache failed before redeem: {e}")

    total_supply_after = await user_client.get_total_supply()
    if int(total_supply_after) == 0:
        print("Step 4 - Skipping redeem: total_supply is 0 (no shares minted)")
    else:
        redeem_amount = "1000"  # Small amount to test the flow
        print(f"Step 4 - Requesting withdrawal of {redeem_amount} shares...")

        try:
            await user_client.redeem(
                redeem_amount,
                AccountId(config.user_account),
                "3000000000000000000000",  # 0.003 NEAR for withdrawal storage
            )
            print("✓ Redeem transaction submitted")

            # Check withdrawal queue state
            pending_id = await user_client.peek_next_pending_withdrawal_id()
            print(f"  Queue head after redeem: {pending_id}")
        except ErrorWrapper as e:
            print(f"⚠ Redeem failed: {e}")

    # === Step 5: Execute withdrawals (flush queue) ===
    if allocator_client and markets:
        market_ids = [m.market_id for m in markets]

        # The vault processes one queued withdrawal per execute_withdrawal() call.
        # Flush the entire queue by repeating: start op -> drive -> wait for idle -> repeat.
        max_cycles = 10
        for cycle in range(max_cycles):
            try:
                # Best-effort cache clearing to avoid reading stale head/op state.
                try:
                    await user_client.clear_view_cache()
                except ErrorWrapper as e:
                    print(f"  ⚠ clear_view_cache failed in flush loop: {e}")

                head = await user_client.peek_next_pending_withdrawal_id()
                if head is None:
                    print("Step 5 - Withdrawal queue empty")
                    break

                print(
                    f"Step 5 - Flushing withdrawal queue (cycle {cycle + 1}/{max_cycles}, head={head})..."
                )

                # If we're in payout, withdrawing_op_id() may be None but current request id is Some.
                # Wait until the vault is truly idle before starting a new withdrawal.
                cycle_deadline = asyncio.get_running_loop().time() + LOOP_TIMEOUT_SECONDS
                timed_out = False
                while True:
                    if asyncio.get_running_loop().time() >= cycle_deadline:
                        print(
                            f"  ⚠ Timed out waiting for idle state before execute_withdrawal (>{LOOP_TIMEOUT_SECONDS}s)"
                        )
                        timed_out = True
                        break
                    withdrawing_op = await user_client.get_withdrawing_op_id()
                    current_req = await user_client.get_current_withdraw_request_id()

                    if withdrawing_op is None and current_req is None:
                        break

                    if withdrawing_op is None and current_req is not None:
                        # Payout in progress; nothing to do but wait for callbacks.
                        await asyncio.sleep(1)
                        continue

                    # Drive an in-progress withdrawing op via market execution.
                    print(f"  Driving withdrawal op_id={withdrawing_op}...")
                    max_iterations = 10
                    for _ in range(max_iterations):
                        op_id = await user_client.get_withdrawing_op_id()
                        if op_id is None:
                            break

                        for market in markets:
                            try:
                                await allocator_client.execute_market_withdrawal(
                                    op_id,
                                    market.market_id,
                                    None,  # batch_limit
                                )
                                print(
                                    f"    Executed withdrawal from market {market.market_id}"
                                )
                            except ErrorWrapper as e:
                                # May fail if market has nothing ready to withdraw.
                                print(
                                    f"    ⚠ execute_market_withdrawal failed for market {market.market_id}: {e}"
                                )

                        # Let the chain progress between iterations.
                        await asyncio.sleep(1)

                    # After driving, wait for payout to finish (if it started).
                    while True:
                        if asyncio.get_running_loop().time() >= cycle_deadline:
                            print(
                                f"  ⚠ Timed out waiting for payout callbacks (>{LOOP_TIMEOUT_SECONDS}s)"
                            )
                            timed_out = True
                            break
                        op_id = await user_client.get_withdrawing_op_id()
                        current_req = (
                            await user_client.get_current_withdraw_request_id()
                        )
                        if op_id is None and current_req is None:
                            break
                        await asyncio.sleep(1)

                if timed_out:
                    break

                # Start the next queued withdrawal (processes the current head).
                try:
                    await allocator_client.execute_withdrawal(market_ids)
                    print("  ✓ Execute withdrawal route submitted")
                except ErrorWrapper as e:
                    print(f"  ⚠ execute_withdrawal failed: {e}")

                # Now wait/drive until the vault becomes idle again.
                while True:
                    if asyncio.get_running_loop().time() >= cycle_deadline:
                        print(
                            f"  ⚠ Timed out waiting for vault to return idle after execute_withdrawal (>{LOOP_TIMEOUT_SECONDS}s)"
                        )
                        timed_out = True
                        break
                    withdrawing_op = await user_client.get_withdrawing_op_id()
                    current_req = await user_client.get_current_withdraw_request_id()

                    if withdrawing_op is None and current_req is None:
                        break

                    if withdrawing_op is None and current_req is not None:
                        await asyncio.sleep(1)
                        continue

                    # withdrawing_op is active
                    max_iterations = 10
                    for _ in range(max_iterations):
                        op_id = await user_client.get_withdrawing_op_id()
                        if op_id is None:
                            break

                        for market in markets:
                            try:
                                await allocator_client.execute_market_withdrawal(
                                    op_id,
                                    market.market_id,
                                    None,
                                )
                                print(
                                    f"    Executed withdrawal from market {market.market_id}"
                                )
                            except ErrorWrapper as e:
                                print(
                                    f"    ⚠ execute_market_withdrawal failed for market {market.market_id}: {e}"
                                )

                        await asyncio.sleep(1)

                if timed_out:
                    break

                # Detect lack of progress to avoid looping forever.
                new_head = await user_client.peek_next_pending_withdrawal_id()
                if new_head == head:
                    print(
                        f"  ⚠ Queue head did not advance (still {new_head}); stopping flush"
                    )
                    break

            except ErrorWrapper as e:
                print(f"⚠ Step 5 flush failed: {e}")
                break
        else:
            print(f"⚠ Step 5 flush did not complete after {max_cycles} cycles")
    else:
        print("Step 5 - Skipping execute withdrawal: no allocator or no markets")

    # === Step 6: Verify final state ===
    final_assets = await user_client.get_total_assets()
    final_supply = await user_client.get_total_supply()
    final_idle = await user_client.get_idle_balance()
    print("Step 6 - Final state:")
    print(f"  Total assets: {final_assets}")
    print(f"  Total supply: {final_supply}")
    print(f"  Idle balance: {final_idle}")

    print()
    print("✓ Happy path flow complete!")


def main():
    print("=" * 60)
    print("Templar Vault Client Python Smoke Test")
    print("=" * 60)
    print()

    # === Sync construction tests (always run) ===
    test_imports()
    test_key_credential_creation()
    test_key_pool_config_defaults()
    test_vault_client_config()
    test_invalid_credential_rejected()
    test_empty_credentials_rejected()

    # === Async construction tests (always run) ===
    asyncio.run(test_client_creation_with_valid_key())
    asyncio.run(test_vault_client_creation())
    asyncio.run(test_vault_client_single_key_default())

    # === Integration tests (require VAULT_ACCOUNT env var) ===
    if os.environ.get("VAULT_ACCOUNT"):
        print()
        print("=" * 60)
        print("Integration Tests (live vault)")
        print("=" * 60)
        print()

        config = SmokeTestConfig.from_env()
        print("Configuration:")
        print(f"  Vault: {config.vault_account}")
        print(f"  RPC URL: {config.rpc_url}")
        print(f"  RPC API Key: {'set' if config.rpc_api_key else 'not set'}")
        print()

        # View methods test (read-only, always safe)
        asyncio.run(test_view_methods(config))

        # Full happy path (requires funded accounts)
        if "--full" in sys.argv or os.environ.get("RUN_FULL_TESTS"):
            print()
            print("-" * 60)
            print("Full Happy Path Test")
            print("-" * 60)
            print()
            asyncio.run(test_happy_path_flow(config))
    else:
        print()
        print("⚠ Set VAULT_ACCOUNT to run integration tests")

    print()
    print("=" * 60)
    print("All smoke tests passed!")
    print("=" * 60)


if __name__ == "__main__":
    main()
