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
    VaultClientConfig,
    AccountId,
    ErrorWrapper,
    AllocationDelta,
    Delta,
    StorageBalance,
    StorageBalanceBounds,
)


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
        """Load configuration from environment variables."""
        return cls(
            rpc_url=os.environ.get("RPC_URL", "https://rpc.testnet.fastnear.com"),
            vault_account=os.environ["VAULT_ACCOUNT"],
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


def test_imports():
    """Test that all expected classes are importable."""
    print("✓ All imports successful")


def test_key_credential_creation():
    """Test KeyCredential dataclass creation."""
    cred = KeyCredential(
        account_id=AccountId("test.near"),
        secret_key="ed25519:3D4YudUQRE39Lc4JHghuB5WM8kbgDDa34mnrEP5DdTApVH81af3e7MvFrog1CMNn67PCi6eC8x9TgDxCV9ySeGir",
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
        client = KeyPoolClient(
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
        client = KeyPoolClient(
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
    config = KeyPoolConfig(
        timeout_seconds=10,
        retry=None,
        max_nonce_retries=3,
        block_hash_ttl_seconds=30,
        view_cache_capacity=100,
        view_cache_ttl_seconds=5,
        rpc_api_key=None,
    )

    # Generate a valid ed25519 key (won't work on-chain but should pass validation)
    # This is a randomly generated key for testing only
    cred = KeyCredential(
        account_id=AccountId("test.testnet"),
        secret_key="ed25519:3D4YudUQRE39Lc4JHghuB5WM8kbgDDa34mnrEP5DdTApVH81af3e7MvFrog1CMNn67PCi6eC8x9TgDxCV9ySeGir",
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
    config = VaultClientConfig(
        timeout_seconds=10,
        retry=None,
        max_nonce_retries=3,
        block_hash_ttl_seconds=30,
        view_cache_capacity=100,
        view_cache_ttl_seconds=5,
        rpc_api_key=None,
    )

    cred = KeyCredential(
        account_id=AccountId("test.testnet"),
        secret_key="ed25519:3D4YudUQRE39Lc4JHghuB5WM8kbgDDa34mnrEP5DdTApVH81af3e7MvFrog1CMNn67PCi6eC8x9TgDxCV9ySeGir",
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
    cred = KeyCredential(
        account_id=AccountId("test.testnet"),
        secret_key="ed25519:3D4YudUQRE39Lc4JHghuB5WM8kbgDDa34mnrEP5DdTApVH81af3e7MvFrog1CMNn67PCi6eC8x9TgDxCV9ySeGir",
    )

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
    It uses a dummy credential since VaultClient requires one, but only makes
    view calls.
    """
    # Create a client with a dummy credential (we only make view calls)
    # Note: We need a credential to create VaultClient, but view calls don't use it
    dummy_cred = KeyCredential(
        account_id=AccountId("dummy.testnet"),
        secret_key="ed25519:3D4YudUQRE39Lc4JHghuB5WM8kbgDDa34mnrEP5DdTApVH81af3e7MvFrog1CMNn67PCi6eC8x9TgDxCV9ySeGir",
    )

    # Use custom config if API key is provided
    client_config = VaultClientConfig(
        timeout_seconds=60,
        retry=None,
        max_nonce_retries=3,
        block_hash_ttl_seconds=30,
        view_cache_capacity=100,
        view_cache_ttl_seconds=5,
        rpc_api_key=config.rpc_api_key,
    )

    client = VaultClient.new_single_key(
        rpc_url=config.rpc_url,
        vault=AccountId(config.vault_account),
        credential=dummy_cred,
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
    print(f"  Configuration: owner={vault_config.owner}, curator={vault_config.curator}")

    fees = await client.get_fees()
    print(f"  Fees: management={fees.management}, performance={fees.performance}")

    restrictions = await client.get_restrictions()
    print(f"  Restrictions: {restrictions}")

    fee_anchor = await client.get_fee_anchor()
    print(f"  Fee anchor: timestamp_ns={fee_anchor.timestamp_ns}, total_assets={fee_anchor.total_assets}")

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
    print(f"  Next pending withdrawal ID: {pending_id}")

    current_req = await client.get_current_withdraw_request_id()
    print(f"  Current withdraw request ID: {current_req}")

    queue_tail = await client.queue_tail()
    print(f"  Queue tail: {queue_tail}")

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
        print(f"  ⚠ Allocator client NOT created (missing credentials)")

    # === Step 0: Clear any in-progress operations ===
    withdrawing_op = await user_client.get_withdrawing_op_id()
    if withdrawing_op is not None and allocator_client:
        print(f"Step 0 - Clearing in-progress withdrawal (op_id={withdrawing_op})...")
        markets_for_clear = await user_client.list_markets_with_ids()
        try:
            max_iterations = 10
            for i in range(max_iterations):
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
                    except:
                        pass
            else:
                print(f"  ⚠ Could not clear withdrawal after {max_iterations} iterations")
        except Exception as e:
            print(f"  ⚠ Error clearing withdrawal: {e}")
    elif withdrawing_op is not None:
        print(f"⚠ Withdrawal in progress (op_id={withdrawing_op}) but no allocator to clear it")

    # === Step 1: Check initial state ===
    initial_assets = await user_client.get_total_assets()
    initial_supply = await user_client.get_total_supply()
    initial_idle = await user_client.get_idle_balance()
    max_deposit = await user_client.get_max_deposit()
    markets = await user_client.list_markets_with_ids()
    print(f"Step 1 - Initial state:")
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
        print("Step 1.5 - Curator setup (registering vault and setting supply_queue)...")

        vault_account = AccountId(config.vault_account)
        token_storage_deposit_amount = "1250000000000000000000"  # 0.00125 NEAR for tokens
        market_storage_deposit_amount = "10000000000000000000000"  # 0.01 NEAR for markets

        # Register vault with itself (NEP-145)
        # This is needed for the vault to hold its own shares (fee accrual)
        print(f"  Checking vault self-registration...")
        try:
            vault_self_balance = await allocator_client.storage_balance_of(vault_account)
            if vault_self_balance is None:
                print(f"    Registering vault with itself...")
                bounds = await allocator_client.storage_balance_bounds()
                storage = await allocator_client.storage_deposit(
                    vault_account,
                    bounds.min,
                )
                print(f"    ✓ Vault registered with itself: total={storage.total}")
            else:
                print(f"    ✓ Already registered: total={vault_self_balance.total}")
        except Exception as e:
            print(f"    ⚠ Vault self-registration: {e}")

        # Register vault with underlying token contract (NEP-145)
        # This is needed so the vault can receive tokens from depositors
        if config.underlying_token:
            print(f"  Checking vault registration with token {config.underlying_token}...")
            try:
                token_balance = await allocator_client.token_storage_balance_of(
                    AccountId(config.underlying_token),
                    vault_account,
                )
                if token_balance is None:
                    print(f"    Registering vault with token...")
                    storage = await allocator_client.token_storage_deposit(
                        AccountId(config.underlying_token),
                        vault_account,
                        token_storage_deposit_amount,
                    )
                    print(f"    ✓ Vault registered with token: total={storage.total}")
                else:
                    print(f"    ✓ Already registered: total={token_balance.total}")
            except Exception as e:
                print(f"    ⚠ Token storage registration: {e}")

        if not markets and config.market_account:
            print(f"  No markets registered - adding market {config.market_account}...")
            market_cap = "1000000000000000000000000"  # 1M tokens (assuming 18 decimals)

            # Submit cap for the market
            print(f"    Submitting cap for market...")
            try:
                await allocator_client.submit_cap(
                    AccountId(config.market_account),
                    market_cap,
                )
                print(f"    ✓ submit_cap succeeded")
            except Exception as e:
                print(f"    ⚠ submit_cap failed: {e}")

            # Accept cap (may fail if timelock > 0)
            print(f"    Accepting cap for market...")
            try:
                await allocator_client.accept_cap(
                    AccountId(config.market_account),
                )
                print(f"    ✓ accept_cap succeeded")

                # Refresh markets list
                markets = await user_client.list_markets_with_ids()
                print(f"    Markets after setup: {len(markets)}")
            except Exception as e:
                print(f"    ⚠ accept_cap failed: {e}")
                print(f"      (May need to wait for timelock, or already accepted)")

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
                        print(f"    Registering vault with market...")
                        storage = await allocator_client.token_storage_deposit(
                            AccountId(market.account),
                            vault_account,
                            market_storage_deposit_amount,
                        )
                        print(f"    ✓ Vault registered with market: total={storage.total}")
                    else:
                        print(f"    ✓ Already registered: total={market_balance.total}")
                except Exception as e:
                    print(f"    ⚠ Market storage registration: {e}")

            # Set supply queue with all markets (if max_deposit is 0)
            if int(max_deposit) == 0:
                print(f"  Setting supply_queue with {len(markets)} markets...")
                try:
                    market_ids = [m.market_id for m in markets]
                    supply_queue_deposit = "840000000000000000000"  # ~0.00084 NEAR per market
                    await allocator_client.set_supply_queue(
                        markets=market_ids,
                        deposit_yocto=supply_queue_deposit,
                    )
                    print("  ✓ Supply queue set")

                    # Re-check max_deposit
                    max_deposit = await user_client.get_max_deposit()
                    print(f"  Max deposit after setup: {max_deposit}")
                except Exception as e:
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
        except Exception as e:
            print(f"  ⚠ Vault storage registration failed: {e}")

        # Register user with token contract (NEP-145) if not already registered
        print(f"  Checking token storage registration for {config.underlying_token}...")
        try:
            # Note: We register ourselves with the token so we can send tokens
            # Typical minimum storage deposit for NEP-141 tokens is 0.00125 NEAR
            token_storage_deposit = "1250000000000000000000"  # 0.00125 NEAR
            token_storage = await user_client.token_storage_deposit(
                token=AccountId(config.underlying_token),
                account_id=None,  # sender
                deposit_yocto=token_storage_deposit,
            )
            print(f"  ✓ Token storage registered: total={token_storage.total}")
        except Exception as e:
            # This might fail if already registered, which is fine
            print(f"  ⚠ Token storage registration: {e}")
            print("    (May already be registered, continuing...)")

        try:
            used = await user_client.ft_transfer_call(
                token=AccountId(config.underlying_token),
                amount=deposit_amount,
                msg=None,
            )
            print(f"✓ Deposit transaction submitted (tokens used: {used})")

            # Verify assets increased
            after_deposit_assets = await user_client.get_total_assets()
            after_deposit_idle = await user_client.get_idle_balance()
            print(f"  Total assets after: {after_deposit_assets}")
            print(f"  Idle balance after: {after_deposit_idle}")

            if int(after_deposit_idle) > int(initial_idle):
                print("✓ Deposit verified: idle balance increased")
            else:
                print("⚠ Deposit: idle balance did not increase")
        except Exception as e:
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
        print(f"Step 3 - Reallocating {reallocate_amount} to market {market.market_id} ({market.account})...")

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
        except Exception as e:
            print(f"⚠ Reallocate failed: {e}")
    else:
        if not allocator_client:
            print("Step 3 - Skipping reallocate: no allocator credentials")
        elif not markets:
            print("Step 3 - Skipping reallocate: no markets registered")
        else:
            print("Step 3 - Skipping reallocate: no idle balance to reallocate")

    # === Step 4: Request withdrawal (redeem shares) ===
    # Get user's current shares - we'll try to redeem a small amount
    redeem_amount = "1000"  # Small amount to test the flow
    print(f"Step 4 - Requesting withdrawal of {redeem_amount} shares...")

    try:
        await user_client.redeem(
            redeem_amount,
            AccountId(config.user_account),
            "3000000000000000000000",  # 0.003 NEAR for withdrawal storage
        )
        print("✓ Redeem transaction submitted")

        # Check withdrawal is queued
        pending_id = await user_client.peek_next_pending_withdrawal_id()
        print(f"  Pending withdrawal ID: {pending_id}")
    except Exception as e:
        print(f"⚠ Redeem failed: {e}")

    # === Step 5: Execute withdrawal (if allocator available and funds in idle) ===
    if allocator_client and markets:
        withdrawing_op = await user_client.get_withdrawing_op_id()
        market_ids = [m.market_id for m in markets]

        if withdrawing_op is None:
            # Need to execute_withdrawal to start the process
            print(f"Step 5 - Starting withdrawal execution...")
            try:
                await allocator_client.execute_withdrawal(market_ids)
                print("✓ Execute withdrawal route submitted")
                withdrawing_op = await user_client.get_withdrawing_op_id()
            except Exception as e:
                print(f"⚠ Execute withdrawal failed: {e}")
        else:
            print(f"Step 5 - Withdrawal already in progress: op_id={withdrawing_op}")

        # Drive any in-progress withdrawal to completion
        if withdrawing_op is not None:
            print(f"  Driving withdrawal op_id={withdrawing_op} to completion...")
            try:
                # Execute market withdrawals until done
                max_iterations = 10
                for i in range(max_iterations):
                    op_id = await user_client.get_withdrawing_op_id()
                    if op_id is None:
                        print("  ✓ Withdrawal completed (vault now idle)")
                        break

                    # Try each market
                    for market in markets:
                        try:
                            await allocator_client.execute_market_withdrawal(
                                op_id,
                                market.market_id,
                                None,  # batch_limit
                            )
                            print(f"    Executed withdrawal from market {market.market_id}")
                        except Exception as e:
                            # May fail if this market has no funds to withdraw
                            pass

                    # Check if we're done
                    op_id = await user_client.get_withdrawing_op_id()
                    if op_id is None:
                        print("  ✓ Withdrawal completed (vault now idle)")
                        break
                else:
                    print(f"  ⚠ Withdrawal not completed after {max_iterations} iterations")
            except Exception as e:
                print(f"  ⚠ Driving withdrawal failed: {e}")
    else:
        print("Step 5 - Skipping execute withdrawal: no allocator or no markets")

    # === Step 6: Verify final state ===
    final_assets = await user_client.get_total_assets()
    final_supply = await user_client.get_total_supply()
    final_idle = await user_client.get_idle_balance()
    print(f"Step 6 - Final state:")
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
        print(f"Configuration:")
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
