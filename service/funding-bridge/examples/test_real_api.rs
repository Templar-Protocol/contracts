//! Test real NEAR Intents Bridge API

use templar_funding_bridge::bridge::{BridgeClient, MAINNET_BRIDGE_API};

#[tokio::main]
async fn main() {
    println!(
        "Testing NEAR Intents Bridge API at {}\n",
        MAINNET_BRIDGE_API
    );

    let client = BridgeClient::new_mainnet();

    // Test 1: Get supported tokens for Ethereum mainnet
    println!("1. Getting supported tokens for eth:1...");
    match client.get_supported_tokens(&["eth:1".to_string()]).await {
        Ok(tokens) => {
            println!("   Found {} tokens:", tokens.len());
            for token in tokens.iter().take(5) {
                println!(
                    "   - {} ({})",
                    token.asset_name, token.defuse_asset_identifier
                );
                println!("     NEAR token: {}", token.near_token_id);
                println!("     Decimals: {}", token.decimals);
                if let Some(fee) = &token.withdrawal_fee {
                    println!("     Withdrawal fee: {}", fee);
                }
            }
            if tokens.len() > 5 {
                println!("   ... and {} more", tokens.len() - 5);
            }
        }
        Err(e) => println!("   ERROR: {}", e),
    }

    // Test 2: Get deposit address for a test account
    println!("\n2. Getting deposit address for tmplr-liq.near on eth:1...");
    match client.get_deposit_address("tmplr-liq.near", "eth:1").await {
        Ok(result) => {
            println!("   Deposit address: {}", result.address);
            println!("   Chain: {}", result.chain);
        }
        Err(e) => println!("   ERROR: {}", e),
    }

    // Test 3: Get deposit address for Arbitrum
    println!("\n3. Getting deposit address for tmplr-liq.near on eth:42161 (Arbitrum)...");
    match client
        .get_deposit_address("tmplr-liq.near", "eth:42161")
        .await
    {
        Ok(result) => {
            println!("   Deposit address: {}", result.address);
            println!("   Chain: {}", result.chain);
        }
        Err(e) => println!("   ERROR: {}", e),
    }

    // Test 4: Find specific token
    println!("\n4. Finding USDC token info for eth:1...");
    match client.find_token("USDC", "eth:1").await {
        Ok(Some(token)) => {
            println!("   Found USDC:");
            println!("   - Defuse ID: {}", token.defuse_asset_identifier);
            println!("   - NEAR token: {}", token.near_token_id);
            println!("   - Decimals: {}", token.decimals);
        }
        Ok(None) => println!("   USDC not found"),
        Err(e) => println!("   ERROR: {}", e),
    }

    // Test 5: Health check
    println!("\n5. Bridge API health check...");
    let healthy = client.health_check().await;
    println!("   Bridge API reachable: {}", healthy);

    println!("\nAll tests completed!");
}
