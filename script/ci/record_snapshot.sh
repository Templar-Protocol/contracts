#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
source "$SCRIPT_DIR/utils.sh"

parse_args "--network:NETWORK,--registry:REGISTRY_ID,--log-file:LOG_FILE,--delay:DELAY,--account:ACCOUNT_ID,--private-key:PRIVATE_KEY,--page-size:PAGE_SIZE" "$@"

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

if [ -z "$REGISTRY_ID" ]; then
    REGISTRY_ID="v1.tmplr.near"
fi

if [ -z "$LOG_FILE" ]; then
    LOG_FILE="/tmp/near-snapshots-$(date +%Y-%m-%d).log"
fi

if [ -z "$DELAY" ]; then
    DELAY=1
fi

if [ -z "$ACCOUNT_ID" ]; then
    echo "ERROR: --account is required for transaction signing"
    exit 1
fi

if [ -z "$PRIVATE_KEY" ]; then
    echo "ERROR: --private-key is required for transaction signing"
    exit 1
fi

if [ -z "$PAGE_SIZE" ]; then
    PAGE_SIZE=100
fi

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() {
    echo -e "[$(date '+%Y-%m-%d %H:%M:%S')] $1" | tee -a "$LOG_FILE"
}

get_deployed_contracts() {
    log "${YELLOW}Fetching deployed contracts from $REGISTRY_ID...${NC}"

    local all_contracts=""
    local from_index=0
    local total_fetched=0

    while true; do
        log "Fetching batch starting from index $from_index (page size: $PAGE_SIZE)..."

        local contracts_json
        contracts_json=$(near contract call-function as-read-only "$REGISTRY_ID" \
            get_all_deployments \
            json-args "{\"from_index\": $from_index, \"limit\": $PAGE_SIZE}" \
            network-config "$NETWORK" 2>/dev/null)

        if [ -z "$contracts_json" ] && [ $from_index -eq 0 ]; then
            log "${YELLOW}Pagination failed, trying without pagination...${NC}"
            contracts_json=$(near contract call-function as-read-only "$REGISTRY_ID" \
                get_all_deployments \
                json-args '{}' \
                network-config "$NETWORK" 2>/dev/null)

            if [ -z "$contracts_json" ]; then
                log "${RED}ERROR: Failed to fetch contracts from $REGISTRY_ID${NC}"
                return 1
            fi

            echo "$contracts_json" | jq -r '.[] | if type == "string" then . else .contract_id // .address // .id end' 2>/dev/null
            return 0
        fi

        if [ -z "$contracts_json" ]; then
            log "${RED}ERROR: Failed to fetch contracts from $REGISTRY_ID at index $from_index${NC}"
            return 1
        fi

        local batch_count
        batch_count=$(echo "$contracts_json" | jq '. | length' 2>/dev/null)

        if [ -z "$batch_count" ] || [ "$batch_count" -eq 0 ]; then
            log "No more contracts found. Pagination complete."
            break
        fi

        local parsed_batch
        parsed_batch=$(echo "$contracts_json" | jq -r '.[] | if type == "string" then . else .contract_id // .address // .id end' 2>/dev/null)

        if [ -n "$parsed_batch" ]; then
            if [ -z "$all_contracts" ]; then
                all_contracts="$parsed_batch"
            else
                all_contracts="$all_contracts"$'\n'"$parsed_batch"
            fi
        fi

        total_fetched=$((total_fetched + batch_count))
        log "Fetched $batch_count contracts in this batch (total: $total_fetched)"

        if [ "$batch_count" -lt "$PAGE_SIZE" ]; then
            log "Reached end of contracts (batch size < page size)"
            break
        fi

        from_index=$((from_index + PAGE_SIZE))
        sleep 0.5
    done

    if [ -z "$all_contracts" ]; then
        log "${RED}ERROR: No contracts found in registry${NC}"
        return 1
    fi

    echo "$all_contracts"
}

call_snapshot() {
    local contract_id="$1"

    log "Calling get_current_snapshot on $contract_id..."

    local result
    result=$(near contract call-function as-transaction "$contract_id" \
        get_current_snapshot \
        json-args '{}' \
        prepaid-gas '30.0 Tgas' \
        attached-deposit '0 NEAR' \
        sign-as "$ACCOUNT_ID" \
        network-config "$NETWORK" \
        sign-with-plaintext-private-key "$PRIVATE_KEY" \
        send 2>&1)

    local exit_code=$?

    if [ $exit_code -eq 0 ]; then
        log "${GREEN}Successfully called get_current_snapshot on $contract_id${NC}"
        return 0
    else
        log "${RED}Failed to call get_current_snapshot on $contract_id${NC}"
        log "Error: $result"
        return 1
    fi
}

main() {
    log "${GREEN}Starting market snapshot collection...${NC}"
    log "Network: $NETWORK"
    log "Registry: $REGISTRY_ID"
    log "Account: $ACCOUNT_ID"
    log "Page size: $PAGE_SIZE"
    log "Log file: $LOG_FILE"

    local contracts
    contracts=$(get_deployed_contracts)
    if [ -z "$contracts" ]; then
        log "${RED}CRITICAL: Could not fetch contracts from registry${NC}"
        exit 1
    fi

    local contract_count
    contract_count=$(echo "$contracts" | wc -l)
    log "${GREEN}Found $contract_count contracts to process${NC}"
    
    local success_count=0
    local failure_count=0

    while IFS= read -r contract; do
        if [ -n "$contract" ]; then
            if call_snapshot "$contract"; then
                ((success_count++))
            else
                ((failure_count++))
            fi

            sleep "$DELAY"
        fi
    done <<< "$contracts"

    log "${GREEN}Snapshot collection completed!${NC}"
    log "Successfully processed: $success_count contracts"
    if [ $failure_count -gt 0 ]; then
        log "${YELLOW}Failed to process: $failure_count contracts${NC}"
    fi

    echo "Done"

    if [ $failure_count -gt 0 ]; then
        exit 1
    fi
}

main "$@"
