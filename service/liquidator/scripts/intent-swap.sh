#!/bin/bash
set -e

# ==============================================================================
# 1-Click API Token Swap Script
# ==============================================================================
# Swaps NEP-141 tokens to NEP-245 tokens using NEAR Intents 1-Click API
#
# USAGE:
#   export NEAR_PRIVATE_KEY=ed25519:YOUR_KEY
#   ./intent-swap.sh
#
# REQUIREMENTS:
#   - near-cli (JavaScript version, not near-cli-rs)
#   - curl, jq
#
# CRITICAL: depositType Configuration
#   For NEP-141→NEP-245 swaps:
#     depositType="ORIGIN_CHAIN"   (tokens are on NEAR blockchain)
#     refundType="ORIGIN_CHAIN"    (refunds go back to NEAR)
#     recipientType="INTENTS"      (receive as NEP-245 in intents.near)
#
#   For NEP-245→NEP-245 swaps:
#     depositType="INTENTS"        (tokens already in intents.near)
#     refundType="INTENTS"
#     recipientType="INTENTS"
#
#   ⚠️  Using wrong depositType causes silent failures - tokens transfer but
#       are never processed by 1-Click API!
#
# TOKEN FORMATS:
#   NEP-141: nep141:CONTRACT_ID
#   NEP-245: nep245:CONTRACT_ID:TOKEN_ID
#
# SWAP WORKFLOW:
#   1. Request quote from 1-Click API (get deposit address)
#   2. Register storage deposit for deposit address
#   3. Transfer NEP-141 tokens to deposit address
#   4. Submit transaction hash to 1-Click API
#   5. Poll status until SUCCESS/REFUNDED/FAILED
#
# ==============================================================================

# Configuration
NEAR_ACCOUNT="your-near-account.near"
FROM_TOKEN="nep141:17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1"
TO_TOKEN="nep245:v2_1.omni.hot.tg:1100_111bzQBB65GxAPAVoxqmMcgYo5oS3txhqs1Uh1cgahKQUeTUq1TJu"
AMOUNT="5000000"
SLIPPAGE_BPS="300"
REFERRAL="templar-manual"
API_BASE_URL="https://1click.chaindefuser.com"
NETWORK="mainnet"
RPC_URL="https://free.rpc.fastnear.com"

# Extract contract ID from FROM_TOKEN
FROM_CONTRACT=$(echo "$FROM_TOKEN" | cut -d':' -f2)

# Check required environment variables
if [ -z "$NEAR_PRIVATE_KEY" ]; then
    echo "ERROR: NEAR_PRIVATE_KEY environment variable not set"
    echo "Example: export NEAR_PRIVATE_KEY=ed25519:YOUR_KEY_HERE"
    exit 1
fi

echo "=================================================="
echo "1-Click API Token Swap"
echo "=================================================="
echo "Network: $NETWORK"
echo "Account: $NEAR_ACCOUNT"
echo "From: $FROM_TOKEN"
echo "To: $TO_TOKEN"
echo "Amount: $AMOUNT"
echo "Slippage: $SLIPPAGE_BPS bps (3%)"
echo "=================================================="
echo ""

# ==============================================================================
# Phase 1: Request Quote
# ==============================================================================
echo "Phase 1: Requesting quote..."

DEADLINE=$(date -u -v +10M '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null || date -u -d '+10 minutes' '+%Y-%m-%dT%H:%M:%SZ')

QUOTE_PAYLOAD=$(cat <<EOF
{
  "dry": false,
  "depositMode": "SIMPLE",
  "swapType": "EXACT_INPUT",
  "slippageTolerance": $SLIPPAGE_BPS,
  "originAsset": "$FROM_TOKEN",
  "depositType": "ORIGIN_CHAIN",
  "destinationAsset": "$TO_TOKEN",
  "amount": "$AMOUNT",
  "refundTo": "$NEAR_ACCOUNT",
  "refundType": "ORIGIN_CHAIN",
  "recipient": "$NEAR_ACCOUNT",
  "recipientType": "INTENTS",
  "deadline": "$DEADLINE",
  "referral": "$REFERRAL",
  "quoteWaitingTimeMs": 5000
}
EOF
)

QUOTE_RESPONSE=$(curl -s -X POST "$API_BASE_URL/v0/quote" \
  -H "Content-Type: application/json" \
  -d "$QUOTE_PAYLOAD")

DEPOSIT_ADDRESS=$(echo "$QUOTE_RESPONSE" | jq -r '.quote.depositAddress')
EXPECTED_OUTPUT=$(echo "$QUOTE_RESPONSE" | jq -r '.quote.amountOutFormatted')
MIN_OUTPUT=$(echo "$QUOTE_RESPONSE" | jq -r '.quote.minAmountOut')
TIME_ESTIMATE=$(echo "$QUOTE_RESPONSE" | jq -r '.quote.timeEstimate')

if [ "$DEPOSIT_ADDRESS" = "null" ] || [ -z "$DEPOSIT_ADDRESS" ]; then
    echo "✗ Failed to get quote"
    echo "$QUOTE_RESPONSE" | jq .
    exit 1
fi

echo "✓ Quote received:"
echo "  Deposit address: $DEPOSIT_ADDRESS"
echo "  Expected output: $EXPECTED_OUTPUT"
echo "  Minimum output: $MIN_OUTPUT"
echo "  Time estimate: ${TIME_ESTIMATE}s"
echo ""

# ==============================================================================
# Phase 2: Transfer Tokens
# ==============================================================================
echo "Phase 2: Transferring tokens..."

# Register storage deposit (may fail if already registered - that's OK)
echo "Registering deposit address..."
near contract call-function as-transaction "$FROM_CONTRACT" storage_deposit \
  json-args "{\"account_id\":\"$DEPOSIT_ADDRESS\",\"registration_only\":true}" \
  prepaid-gas '30 Tgas' \
  attached-deposit '0.00125 NEAR' \
  sign-as "$NEAR_ACCOUNT" \
  network-config "$NETWORK" \
  sign-with-plaintext-private-key "$NEAR_PRIVATE_KEY" send \
  > /dev/null 2>&1 || echo "  (May already be registered)"

echo "✓ Registration completed"
echo "Transferring $AMOUNT tokens..."

# Transfer tokens and capture transaction hash
TRANSFER_OUTPUT=$(near contract call-function as-transaction "$FROM_CONTRACT" ft_transfer \
  json-args "{\"receiver_id\":\"$DEPOSIT_ADDRESS\",\"amount\":\"$AMOUNT\"}" \
  prepaid-gas '30 Tgas' \
  attached-deposit '1 yoctoNEAR' \
  sign-as "$NEAR_ACCOUNT" \
  network-config "$NETWORK" \
  sign-with-plaintext-private-key "$NEAR_PRIVATE_KEY" send 2>&1)

# Extract transaction hash from near-cli output
# Look for "Transaction ID:" line
TX_HASH=$(echo "$TRANSFER_OUTPUT" | grep "Transaction ID:" | awk '{print $NF}')

if [ -z "$TX_HASH" ]; then
    echo "✗ Failed to extract transaction hash"
    echo "$TRANSFER_OUTPUT"
    exit 1
fi

echo "✓ Transfer succeeded!"
echo "  Transaction hash: $TX_HASH"
echo ""

# Wait for finalization
echo "Waiting 5s for finalization..."
sleep 5

# ==============================================================================
# Phase 3: Submit Deposit and Poll Status
# ==============================================================================
echo "Phase 3: Notifying 1-Click and monitoring..."

SUBMIT_PAYLOAD=$(cat <<EOF
{
  "txHash": "$TX_HASH",
  "depositAddress": "$DEPOSIT_ADDRESS",
  "nearSenderAccount": "$NEAR_ACCOUNT"
}
EOF
)

curl -s -X POST "$API_BASE_URL/v0/deposit/submit" \
  -H "Content-Type: application/json" \
  -d "$SUBMIT_PAYLOAD" > /dev/null

echo "✓ Deposit submitted"
echo ""

# Poll for completion
echo "Polling swap status (max 240s)..."
MAX_ATTEMPTS=24
ATTEMPT=1

while [ $ATTEMPT -le $MAX_ATTEMPTS ]; do
    sleep 10
    
    STATUS_RESPONSE=$(curl -s "$API_BASE_URL/v0/status?depositAddress=$DEPOSIT_ADDRESS")
    STATUS=$(echo "$STATUS_RESPONSE" | jq -r '.status')
    
    echo "  [$ATTEMPT/$MAX_ATTEMPTS] Status: $STATUS"
    
    if [ "$STATUS" = "SUCCESS" ]; then
        AMOUNT_OUT=$(echo "$STATUS_RESPONSE" | jq -r '.swapDetails.amountOutFormatted')
        echo ""
        echo "✓ Swap completed successfully!"
        echo "  Output: $AMOUNT_OUT"
        echo ""
        echo "=================================================="
        echo "Swap process completed!"
        echo "=================================================="
        exit 0
    fi
    
    if [ "$STATUS" = "REFUNDED" ] || [ "$STATUS" = "FAILED" ]; then
        REASON=$(echo "$STATUS_RESPONSE" | jq -r '.swapDetails.refundReason // "Unknown"')
        echo ""
        echo "✗ Swap $STATUS"
        echo "  Reason: $REASON"
        exit 1
    fi
    
    ATTEMPT=$((ATTEMPT + 1))
done

echo ""
echo "⚠ Polling timeout - swap may still be processing"
echo ""
echo "📊 To check status:"
echo "  curl -s \"$API_BASE_URL/v0/status?depositAddress=$DEPOSIT_ADDRESS\" | jq"
echo ""
echo "=================================================="
echo "Swap process completed!"
echo "=================================================="
