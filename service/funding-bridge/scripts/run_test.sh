#!/bin/bash
#
# Test Funding Bridge Deposits and Withdrawals
#
# This script tests the funding bridge API by making deposit or withdrawal requests
# to the locally running service.
#
# Usage:
#   ./scripts/run_test.sh <direction> <network> <amount>
#
# Arguments:
#   direction   - deposit or withdraw
#   network     - eth, arbitrum, base, solana, stellar
#   amount      - Amount in USDC (e.g., 1.5 for 1.5 USDC)
#
# Examples:
#   ./scripts/run_test.sh deposit solana 1.0     # Deposit 1 USDC from Solana to NEAR
#   ./scripts/run_test.sh withdraw eth 0.5       # Withdraw 0.5 USDC from NEAR to Ethereum
#   ./scripts/run_test.sh deposit stellar 2.0    # Deposit 2 USDC from Stellar to NEAR
#   ./scripts/run_test.sh deposit arbitrum 10    # Deposit 10 USDC from Arbitrum to NEAR
#

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Default service URL
SERVICE_URL="${SERVICE_URL:-http://localhost:3000}"

# Parse arguments
DIRECTION="$1"
NETWORK="$2"
AMOUNT="$3"

# Show usage
show_usage() {
    echo "Usage: $0 <direction> <network> <amount>"
    echo ""
    echo "Arguments:"
    echo "  direction   deposit or withdraw"
    echo "  network     eth, arbitrum, base, optimism, solana, stellar"
    echo "  amount      Amount in USDC (e.g., 1.0)"
    echo ""
    echo "Examples:"
    echo "  $0 deposit solana 1.0     # Deposit from Solana to NEAR treasury"
    echo "  $0 withdraw eth 0.5       # Withdraw from NEAR treasury to Ethereum"
    echo "  $0 deposit stellar 2.0    # Deposit from Stellar to NEAR treasury"
    echo "  $0 deposit arbitrum 10    # Deposit from Arbitrum to NEAR treasury"
    echo ""
    echo "Note: Withdrawal destinations are configured in .env file:"
    echo "  WITHDRAW_ETH_ADDRESS, WITHDRAW_ARBITRUM_ADDRESS, etc."
    echo ""
    exit 1
}

# Validate arguments
if [ -z "$DIRECTION" ] || [ -z "$NETWORK" ] || [ -z "$AMOUNT" ]; then
    echo -e "${RED}Error: Missing required arguments${NC}"
    show_usage
fi

# Validate direction
case "$DIRECTION" in
    deposit|withdraw)
        ;;
    *)
        echo -e "${RED}Error: Invalid direction '$DIRECTION'${NC}"
        echo "Must be 'deposit' or 'withdraw'"
        exit 1
        ;;
esac

# Validate network and normalize to chain name
case "$NETWORK" in
    eth|ethereum)
        CHAIN_NAME="ethereum"
        ;;
    arb|arbitrum)
        CHAIN_NAME="arbitrum"
        ;;
    base)
        CHAIN_NAME="base"
        ;;
    op|optimism)
        CHAIN_NAME="optimism"
        ;;
    sol|solana)
        CHAIN_NAME="solana"
        ;;
    xlm|stellar)
        CHAIN_NAME="stellar"
        ;;
    *)
        echo -e "${RED}Error: Invalid network '$NETWORK'${NC}"
        echo "Must be 'eth', 'arbitrum', 'base', 'optimism', 'solana', or 'stellar'"
        exit 1
        ;;
esac

# Validate amount is a positive number
if ! [[ "$AMOUNT" =~ ^[0-9]+\.?[0-9]*$ ]]; then
    echo -e "${RED}Error: Invalid amount '$AMOUNT'${NC}"
    echo "Amount must be a positive number"
    exit 1
fi

# Check if service is running
if ! curl -s "$SERVICE_URL/health" > /dev/null 2>&1; then
    echo -e "${RED}Error: Service not responding at $SERVICE_URL${NC}"
    echo "Please start the service first:"
    echo "  ./scripts/run_service.sh"
    exit 1
fi

# Get service health info
echo -e "${CYAN}═══════════════════════════════════════════════════════════${NC}"
echo -e "${CYAN}Service Health Check${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════════════════${NC}"
HEALTH=$(curl -s "$SERVICE_URL/health")
echo "$HEALTH" | python3 -m json.tool 2>/dev/null || echo "$HEALTH"
echo ""

# Check if the chain is available
CHAIN_AVAILABLE=$(echo "$HEALTH" | grep -o "\"name\":\"$CHAIN_NAME\"" | wc -l)
if [ "$CHAIN_AVAILABLE" -eq 0 ] && [ "$DIRECTION" = "deposit" ]; then
    echo -e "${YELLOW}Warning: Chain '$CHAIN_NAME' not listed in available chains${NC}"
    echo -e "${YELLOW}This may indicate the service is not configured for $CHAIN_NAME deposits${NC}"
    echo ""
fi

# Prepare the request based on direction
if [ "$DIRECTION" = "deposit" ]; then
    # Create deposit request
    REQUEST_FILE="/tmp/funding_bridge_test_deposit.json"
    cat > "$REQUEST_FILE" <<EOF
{
  "source_chain": "$CHAIN_NAME",
  "asset": "USDC",
  "amount": "$AMOUNT",
  "dry_run": false
}
EOF

    echo -e "${CYAN}═══════════════════════════════════════════════════════════${NC}"
    echo -e "${GREEN}Testing Deposit${NC}"
    echo -e "${CYAN}═══════════════════════════════════════════════════════════${NC}"
    echo -e "Source Chain: ${BLUE}$CHAIN_NAME${NC}"
    echo -e "Asset:        ${BLUE}USDC${NC}"
    echo -e "Amount:       ${BLUE}$AMOUNT USDC${NC}"
    echo ""

    # Make request
    echo -e "${BLUE}Sending request to POST $SERVICE_URL/deposit...${NC}"
    RESPONSE=$(curl -s -X POST "$SERVICE_URL/deposit" \
        -H "Content-Type: application/json" \
        -d @"$REQUEST_FILE" 2>&1)

    # Clean up temp file
    rm -f "$REQUEST_FILE"

else
    # For withdrawals, destination address comes from service config
    # Convert amount to smallest units (USDC has 6 decimals)
    AMOUNT_INT=$(echo "$AMOUNT * 1000000" | bc | cut -d'.' -f1)

    # Create withdrawal request
    REQUEST_FILE="/tmp/funding_bridge_test_withdraw.json"
    cat > "$REQUEST_FILE" <<EOF
{
  "destination_chain": "$CHAIN_NAME",
  "asset": "USDC",
  "amount": "$AMOUNT_INT",
  "dry_run": false
}
EOF

    echo -e "${CYAN}═══════════════════════════════════════════════════════════${NC}"
    echo -e "${GREEN}Testing Withdrawal${NC}"
    echo -e "${CYAN}═══════════════════════════════════════════════════════════${NC}"
    echo -e "Destination Chain: ${BLUE}$CHAIN_NAME${NC}"
    echo -e "Asset:             ${BLUE}USDC${NC}"
    echo -e "Amount:            ${BLUE}$AMOUNT USDC ($AMOUNT_INT smallest units)${NC}"
    echo ""

    # Make request
    echo -e "${BLUE}Sending request to POST $SERVICE_URL/withdraw...${NC}"
    RESPONSE=$(curl -s -X POST "$SERVICE_URL/withdraw" \
        -H "Content-Type: application/json" \
        -d @"$REQUEST_FILE" 2>&1)

    # Clean up temp file
    rm -f "$REQUEST_FILE"
fi

# Display response
echo ""
echo -e "${CYAN}═══════════════════════════════════════════════════════════${NC}"
echo -e "${CYAN}API Response${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════════════════${NC}"
echo "$RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$RESPONSE"
echo ""

# Parse and check response status
if [ "$DIRECTION" = "deposit" ]; then
    # Check deposit response
    if echo "$RESPONSE" | grep -q '"status":"FAILED"'; then
        echo -e "${RED}❌ Deposit failed${NC}"
        ERROR=$(echo "$RESPONSE" | grep -o '"error":"[^"]*"' | cut -d'"' -f4)
        echo -e "${RED}Error: $ERROR${NC}"
        exit 1
    elif echo "$RESPONSE" | grep -q '"status":"PENDING"'; then
        echo -e "${GREEN}✅ Deposit submitted successfully (pending bridge processing)${NC}"
        TX_HASH=$(echo "$RESPONSE" | grep -o '"source_tx_hash":"[^"]*"' | cut -d'"' -f4)
        BRIDGE_ADDR=$(echo "$RESPONSE" | grep -o '"bridge_deposit_address":"[^"]*"' | cut -d'"' -f4)
        BRIDGE_MEMO=$(echo "$RESPONSE" | grep -o '"bridge_deposit_memo":"[^"]*"' | cut -d'"' -f4)
        if [ -n "$TX_HASH" ]; then
            echo -e "Source TX Hash: ${BLUE}$TX_HASH${NC}"
        fi
        if [ -n "$BRIDGE_ADDR" ]; then
            echo -e "Bridge Address: ${BLUE}$BRIDGE_ADDR${NC}"
        fi
        if [ -n "$BRIDGE_MEMO" ]; then
            echo -e "Bridge Memo:    ${BLUE}$BRIDGE_MEMO${NC}"
            echo -e "${YELLOW}Note: Include this memo when sending to the bridge address${NC}"
        fi
        exit 0
    elif echo "$RESPONSE" | grep -q '"status":"SUBMITTED"'; then
        echo -e "${GREEN}✅ Deposit submitted (awaiting confirmation)${NC}"
        TX_HASH=$(echo "$RESPONSE" | grep -o '"source_tx_hash":"[^"]*"' | cut -d'"' -f4)
        if [ -n "$TX_HASH" ]; then
            echo -e "Source TX Hash: ${BLUE}$TX_HASH${NC}"
        fi
        exit 0
    elif echo "$RESPONSE" | grep -q '"status":"DRY_RUN"'; then
        echo -e "${YELLOW}ℹ️  Dry run mode - no actual transaction executed${NC}"
        BRIDGE_ADDR=$(echo "$RESPONSE" | grep -o '"bridge_deposit_address":"[^"]*"' | cut -d'"' -f4)
        BRIDGE_MEMO=$(echo "$RESPONSE" | grep -o '"bridge_deposit_memo":"[^"]*"' | cut -d'"' -f4)
        if [ -n "$BRIDGE_ADDR" ]; then
            echo -e "Bridge Address: ${BLUE}$BRIDGE_ADDR${NC}"
        fi
        if [ -n "$BRIDGE_MEMO" ]; then
            echo -e "Bridge Memo:    ${BLUE}$BRIDGE_MEMO${NC}"
            echo -e "${CYAN}To complete deposit manually: Send USDC to the bridge address with this memo${NC}"
        fi
        exit 0
    else
        echo -e "${YELLOW}⚠️  Unknown response status${NC}"
        exit 1
    fi
else
    # Check withdrawal response
    if echo "$RESPONSE" | grep -q '"status":"FAILED"'; then
        echo -e "${RED}❌ Withdrawal failed${NC}"
        ERROR=$(echo "$RESPONSE" | grep -o '"error":"[^"]*"' | cut -d'"' -f4)
        echo -e "${RED}Error: $ERROR${NC}"
        exit 1
    elif echo "$RESPONSE" | grep -q '"status":"PENDING"'; then
        echo -e "${GREEN}✅ Withdrawal submitted successfully (pending bridge processing)${NC}"
        TX_HASH=$(echo "$RESPONSE" | grep -o '"source_tx_hash":"[^"]*"' | cut -d'"' -f4)
        if [ -n "$TX_HASH" ]; then
            echo -e "NEAR TX Hash: ${BLUE}$TX_HASH${NC}"
            echo -e "${CYAN}Track status via Bridge API using this transaction hash${NC}"
        fi
        exit 0
    elif echo "$RESPONSE" | grep -q '"status":"COMPLETED"'; then
        echo -e "${GREEN}✅ Withdrawal completed!${NC}"
        SOURCE_TX=$(echo "$RESPONSE" | grep -o '"source_tx_hash":"[^"]*"' | cut -d'"' -f4)
        DEST_TX=$(echo "$RESPONSE" | grep -o '"destination_tx_hash":"[^"]*"' | cut -d'"' -f4)
        if [ -n "$SOURCE_TX" ]; then
            echo -e "NEAR TX Hash: ${BLUE}$SOURCE_TX${NC}"
        fi
        if [ -n "$DEST_TX" ]; then
            echo -e "Destination TX Hash: ${BLUE}$DEST_TX${NC}"
        fi
        exit 0
    else
        echo -e "${YELLOW}⚠️  Unknown response status${NC}"
        exit 1
    fi
fi
