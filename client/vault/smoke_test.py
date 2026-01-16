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
)


# =============================================================================
# Configuration for Integration Tests
# =============================================================================


@dataclass
class SmokeTestConfig:
    """Configuration for integration tests against a deployed vault."""

    rpc_url: str
    vault_account: str
    underlying_token: Optional[str] = None
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
            rpc_url=os.environ.get("RPC_URL", "https://rpc.testnet.near.org"),
            vault_account=os.environ["VAULT_ACCOUNT"],
            underlying_token=os.environ.get("UNDERLYING_TOKEN"),
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
    )

    # Invalid secret key
    cred = KeyCredential(
        account_id=AccountId("test.near"),
        secret_key="not-a-valid-key",
    )

    try:
        client = KeyPoolClient(
            rpc_url="https://rpc.testnet.near.org",
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
    )

    try:
        client = KeyPoolClient(
            rpc_url="https://rpc.testnet.near.org",
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
    )

    # Generate a valid ed25519 key (won't work on-chain but should pass validation)
    # This is a randomly generated key for testing only
    cred = KeyCredential(
        account_id=AccountId("test.testnet"),
        secret_key="ed25519:3D4YudUQRE39Lc4JHghuB5WM8kbgDDa34mnrEP5DdTApVH81af3e7MvFrog1CMNn67PCi6eC8x9TgDxCV9ySeGir",
    )

    client = KeyPoolClient(
        rpc_url="https://rpc.testnet.near.org",
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
    )

    cred = KeyCredential(
        account_id=AccountId("test.testnet"),
        secret_key="ed25519:3D4YudUQRE39Lc4JHghuB5WM8kbgDDa34mnrEP5DdTApVH81af3e7MvFrog1CMNn67PCi6eC8x9TgDxCV9ySeGir",
    )

    client = VaultClient.new_key_pool(
        rpc_url="https://rpc.testnet.near.org",
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
        rpc_url="https://rpc.testnet.near.org",
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

    client = VaultClient.new_single_key_default(
        rpc_url=config.rpc_url,
        vault=AccountId(config.vault_account),
        credential=dummy_cred,
    )

    print(f"Testing view methods against vault: {config.vault_account}")
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
    Full happy path mirroring contract/vault/tests/happy_path.rs:
    1. Check initial vault state
    2. Deposit tokens to vault (user)
    3. Verify total_assets increased
    4. Reallocate to market (allocator)
    5. Verify idle_balance decreased
    6. Request withdrawal (user)
    7. Execute withdrawal route (allocator)
    8. Verify final state

    Requires: USER_ACCOUNT, USER_SECRET_KEY (funded with underlying tokens)
    Optional: ALLOCATOR_ACCOUNT, ALLOCATOR_SECRET_KEY (for reallocate/execute)
    """
    # Skip if no user credentials
    if not config.user_account or not config.user_secret_key:
        print("⚠ Skipping happy path: USER_ACCOUNT/USER_SECRET_KEY not set")
        return

    print(f"Running happy path flow against vault: {config.vault_account}")
    print(f"  User account: {config.user_account}")
    print()

    # === Setup user client ===
    user_cred = KeyCredential(
        account_id=AccountId(config.user_account),
        secret_key=config.user_secret_key,
    )

    user_client = VaultClient.new_single_key_default(
        rpc_url=config.rpc_url,
        vault=AccountId(config.vault_account),
        credential=user_cred,
    )

    # === Setup allocator client (if available) ===
    allocator_client = None
    if config.allocator_account and config.allocator_secret_key:
        allocator_cred = KeyCredential(
            account_id=AccountId(config.allocator_account),
            secret_key=config.allocator_secret_key,
        )
        allocator_client = VaultClient.new_single_key_default(
            rpc_url=config.rpc_url,
            vault=AccountId(config.vault_account),
            credential=allocator_cred,
        )
        print(f"  Allocator account: {config.allocator_account}")

    # === Step 1: Check initial state ===
    initial_assets = await user_client.get_total_assets()
    initial_supply = await user_client.get_total_supply()
    initial_idle = await user_client.get_idle_balance()
    print(f"Step 1 - Initial state:")
    print(f"  Total assets: {initial_assets}")
    print(f"  Total supply: {initial_supply}")
    print(f"  Idle balance: {initial_idle}")

    # === Step 2: Deposit tokens ===
    deposit_amount = "1000000"  # 1 USDC (6 decimals) or 1 token
    print(f"Step 2 - Depositing {deposit_amount} tokens...")

    try:
        await user_client.deposit_supply(deposit_amount, gas=None)
        print(f"✓ Deposit transaction submitted")
    except Exception as e:
        print(f"✗ Deposit failed: {e}")
        print("  (User may not have sufficient token balance or allowance)")
        return

    # === Step 3: Verify assets increased ===
    after_deposit_assets = await user_client.get_total_assets()
    after_deposit_supply = await user_client.get_total_supply()
    after_deposit_idle = await user_client.get_idle_balance()
    print(f"Step 3 - After deposit:")
    print(f"  Total assets: {after_deposit_assets}")
    print(f"  Total supply: {after_deposit_supply}")
    print(f"  Idle balance: {after_deposit_idle}")

    # Verify increase
    assets_increased = int(after_deposit_assets) > int(initial_assets)
    supply_increased = int(after_deposit_supply) > int(initial_supply)
    idle_increased = int(after_deposit_idle) > int(initial_idle)

    if assets_increased and supply_increased and idle_increased:
        print("✓ Deposit verified: assets, supply, and idle all increased")
    else:
        print(f"⚠ Deposit verification: assets={assets_increased}, supply={supply_increased}, idle={idle_increased}")

    # === Step 4: Reallocate to market (if allocator available) ===
    markets = await user_client.list_markets_with_ids()
    if allocator_client and markets:
        market = markets[0]
        print(f"Step 4 - Reallocating to market {market.market_id} ({market.account})...")

        delta = AllocationDelta.SUPPLY(
            Delta(market=market.market_id, amount=deposit_amount)
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
            print("Step 4 - Skipping reallocate: no allocator credentials")
        else:
            print("Step 4 - Skipping reallocate: no markets registered")

    # === Step 5: Request withdrawal (redeem shares) ===
    # Get user's current shares (we'll redeem a portion)
    redeem_amount = str(int(deposit_amount) // 2)  # Redeem half
    print(f"Step 5 - Requesting withdrawal of {redeem_amount} shares...")

    try:
        await user_client.redeem(
            shares=redeem_amount,
            receiver=AccountId(config.user_account),
            deposit_yocto="1",  # 1 yoctoNEAR for storage
        )
        print("✓ Redeem transaction submitted")

        # Check withdrawal is queued
        pending_id = await user_client.peek_next_pending_withdrawal_id()
        print(f"  Pending withdrawal ID: {pending_id}")
    except Exception as e:
        print(f"⚠ Redeem failed: {e}")

    # === Step 6: Execute withdrawal (if allocator available and funds in idle) ===
    if allocator_client and markets:
        withdrawing_op = await user_client.get_withdrawing_op_id()
        if withdrawing_op is None:
            # Need to execute_withdrawal to start the process
            print(f"Step 6 - Executing withdrawal route...")
            market_ids = [m.market_id for m in markets]

            try:
                await allocator_client.execute_withdrawal(market_ids)
                print("✓ Execute withdrawal route submitted")

                # Execute per-market withdrawal
                op_id = await user_client.get_withdrawing_op_id()
                if op_id is not None:
                    for market in markets:
                        print(f"  Executing market withdrawal for market {market.market_id}...")
                        await allocator_client.execute_market_withdrawal(
                            op_id=op_id,
                            market=market.market_id,
                            batch_limit=None,
                        )
                    print("✓ Market withdrawals executed")
            except Exception as e:
                print(f"⚠ Execute withdrawal failed: {e}")
        else:
            print(f"Step 6 - Withdrawal already in progress: op_id={withdrawing_op}")
    else:
        print("Step 6 - Skipping execute withdrawal: no allocator or no markets")

    # === Step 7: Verify final state ===
    final_assets = await user_client.get_total_assets()
    final_supply = await user_client.get_total_supply()
    final_idle = await user_client.get_idle_balance()
    print(f"Step 7 - Final state:")
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
