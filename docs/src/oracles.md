# Oracle System Documentation

Templar Protocol relies on external price oracles to determine asset values for collateralization and liquidation calculations.

## Oracle Architecture

### Primary Oracle Provider
- **Pyth Network**: Primary price feed provider
- **Oracle Contract**: <TBD> (mainnet) / `pyth-oracle.testnet` (testnet)
- **Documentation**: [Pyth Network Documentation](https://docs.pyth.network/)

### LST Oracle Adapter
For Liquid Staking Tokens (LSTs), Templar uses a custom oracle adapter:
- **Purpose**: Transforms underlying asset prices using redemption rates
- **Contract**: <TBD>
- **Functionality**: Applies LST redemption rates to base asset prices

## Price Feed Configuration

### Market Oracle Settings
Each market is configured with specific oracle parameters:

```rust
pub struct PriceOracleConfiguration {
    /// Account ID of the oracle contract
    pub account_id: AccountId,
    /// Price identifier for borrow asset  
    pub borrow_asset_price_id: PriceIdentifier,
    /// Borrow asset decimals for price conversion
    pub borrow_asset_decimals: u8,
    /// Price identifier for collateral asset
    pub collateral_asset_price_id: PriceIdentifier,
    /// Collateral asset decimals for price conversion
    pub collateral_asset_decimals: u8,
    /// Maximum acceptable price age in seconds
    pub price_maximum_age_s: u32,
}
```

### Price Identifiers
Price feeds are identified using Pyth Network's standardized price IDs:
- **USDT**: `1fc18861232290221461220bd4e2acd1dcdfbc89c84092c93c18bdc7756c1588` (testnet)
- **NEAR**: `27e867f0f4f61076456d1a73b14c7edc1cf5cef4f4d6193a33424288f11bd0f4` (testnet)

## Update Frequency and Freshness

### Pyth Network Updates
- **Update Frequency**: Continuously updated (sub-second intervals)
- **On-Chain Updates**: Pulled on-demand by protocol operations
- **Price Staleness**: Configurable maximum age per market (typically 60 seconds)

### Price Validation
Markets validate price freshness before use:
```rust
// Reject prices older than configured maximum age
pub price_maximum_age_s: u32,
```

If prices are stale:
- Operations that require prices (borrow, liquidate) will fail
- Users must wait for fresh price data
- No fallback to potentially manipulated prices

## Oracle Security Measures

### Price Manipulation Protection

1. **Confidence Intervals**: Pyth prices include confidence bands
2. **Multiple Data Sources**: Pyth aggregates from multiple price providers
3. **Outlier Detection**: Automatic filtering of anomalous price data
4. **Time-Weighted Averages**: EMA (Exponentially Weighted Moving Average) prices used

### Circuit Breakers
- **Maximum Age Limits**: Reject stale price data
- **Confidence Thresholds**: Reject prices with excessive uncertainty
- **Price Deviation Limits**: (Future enhancement) Reject extreme price movements

## Oracle Monitoring and Maintenance

### Available Monitoring Tools
The protocol provides tools for monitoring:

1. **Price Feed Health**
   - Oracle response monitoring via bot infrastructure
   - Price staleness validation in market operations
   - Manual price feed verification procedures

2. **Price Validation**
   - Built-in price age limits in market contracts
   - Confidence interval validation from Pyth
   - Manual cross-validation with external sources

### Manual Monitoring Procedures

<TBD>

### Emergency Response

#### Oracle Failure Scenarios
1. **Temporary Outage**: 
   - Operations requiring prices will pause
   - Users can still withdraw existing positions
   - No new borrows or liquidations until oracle recovery

2. **Price Manipulation Attack**:
   - Markets will reject stale prices automatically
   - Manual review of suspicious price movements required
   - Coordination with oracle provider for investigation needed

3. **Oracle Provider Issues**:
   - Alternative oracle providers could be evaluated
   - Market migration would require new deployments
   - Communication with affected users through official channels

## LST Oracle Adapter Details

### Functionality
The LST oracle adapter enhances base oracle functionality:

```rust
pub struct PriceTransformer {
    pub multiplier: Decimal,      // Conversion multiplier
    pub shift: i8,               // Decimal shift adjustment  
}
```

### Supported Transformations
- **LST Price Calculation**: `base_price * redemption_rate`
- **Decimal Normalization**: Price scaling for different asset decimals
- **Rate Updates**: Real-time redemption rate queries

### Example: stNEAR Price Calculation
1. Fetch NEAR price from Pyth oracle
2. Query stNEAR redemption rate from staking contract
3. Calculate: `stNEAR_price = NEAR_price * redemption_rate`

## Oracle Integration Testing

### Available Tests
1. **LST Oracle Integration**: Tests LST oracle adapter functionality
2. **Price Transformation**: Tests price conversion and decimal handling
3. **Market Integration**: Oracle configuration used in market liquidation tests

### Mock Oracle Contract
A mock oracle contract exists at `mock/oracle/` providing:
- Controllable price feeds for testing
- Basic oracle interface simulation
- Integration with test infrastructure

## Oracle Provider Diversification

### Current Setup
- **Primary**: Pyth Network
- **Backup**: None currently deployed

### Future Enhancements
1. **Multiple Oracle Support**: Integrate additional oracle providers
2. **Price Aggregation**: Combine prices from multiple sources
3. **Fallback Mechanisms**: Automatic failover to backup oracles
4. **Price Deviation Alerts**: Detect inconsistencies between providers

## Gas Optimization

### Oracle Call Costs

<TBD>

### Optimization Strategies
- **Batch Price Queries**: Retrieve multiple prices in single call
- **Caching**: Store recent prices to reduce oracle calls
- **Gas Estimation**: Dynamic gas allocation based on oracle complexity

## Compliance and Auditing

<TBD>
