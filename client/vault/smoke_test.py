#!/usr/bin/env python3
"""Smoke test for templar_vault_client Python bindings."""

import asyncio
import sys

# Import the generated bindings
from templar_vault_client import (
    KeyPoolClient,
    KeyPoolConfig,
    KeyCredential,
    VaultClient,
    VaultClientConfig,
    AccountId,
    ErrorWrapper,
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


def main():
    print("=" * 50)
    print("Templar Vault Client Python Smoke Test")
    print("=" * 50)
    print()

    # Sync tests
    test_imports()
    test_key_credential_creation()
    test_key_pool_config_defaults()
    test_vault_client_config()
    test_invalid_credential_rejected()
    test_empty_credentials_rejected()

    # Async tests
    asyncio.run(test_client_creation_with_valid_key())
    asyncio.run(test_vault_client_creation())
    asyncio.run(test_vault_client_single_key_default())

    print()
    print("=" * 50)
    print("All smoke tests passed!")
    print("=" * 50)


if __name__ == "__main__":
    main()
