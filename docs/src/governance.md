# Protocol Governance and Administrative Controls

This document outlines the current administrative structure and governance controls of Templar Protocol.

## Contract Upgradeability

### Registry Contract
- **Upgrade Status**: Immutable once deployed
- **Owner Account**: Registry is its own owner (verified in [registry/src/lib.rs](https://github.com/Templar-Protocol/contracts/blob/dev/contract/registry/src/lib.rs))
- **Upgrade Mechanism**: No upgrade mechanism implemented
- **Owner Functions**: Limited to version management and market deployment only

### Market Contracts
- **Upgrade Status**: Immutable once deployed
- **Deployment**: Markets are deployed through the registry with immutable code
- **Version Control**: New market versions can be deployed through registry
- **Existing Markets**: Cannot be upgraded; users must migrate to new versions

## Administrative Roles and Capabilities

### Registry Owner Capabilities
The registry owner account has the following administrative privileges (verified in [registry/src/lib.rs](https://github.com/Templar-Protocol/contracts/blob/dev/contract/registry/src/lib.rs)):

1. **Version Management**
   - Add new market contract versions: [`add_version()`](https://github.com/Templar-Protocol/contracts/blob/dev/contract/registry/src/lib.rs)
   - Remove contract versions: [`remove_version()`](https://github.com/Templar-Protocol/contracts/blob/dev/contract/registry/src/lib.rs)
   - Deploy new markets: [`deploy_market()`](https://github.com/Templar-Protocol/contracts/blob/dev/contract/registry/src/lib.rs)

2. **Market Deployment**
   - Deploy markets with specific configurations
   - Set initial full-access keys for deployed markets  
   - Control market naming and sub-account creation

**Note**: Registry contract itself has **no upgrade mechanism** - only version and deployment management functions exist.

### Market Contract Administration
Once deployed, market contracts have **no admin functions** (verified in [market external interface](https://github.com/Templar-Protocol/contracts/blob/dev/common/src/market/external.rs)):
- Markets operate autonomously based on initial configuration
- No ability to pause, upgrade, or modify market parameters
- All operations follow predetermined protocol rules

## Access Control Implementation

### Registry Access Control
Access control implementation (source: [registry/src/lib.rs](https://github.com/Templar-Protocol/contracts/blob/dev/contract/registry/src/lib.rs)):
```rust
// Only owner can add versions
#[payable]
pub fn add_version(&mut self, version_key: String, code: Vec<u8>) {
    self.assert_owner();  // Validates caller is owner
    // ... version addition logic
}

// Only owner can deploy markets
#[payable] 
pub fn deploy_market(&mut self, ...) {
    self.assert_owner();  // Validates caller is owner
    // ... deployment logic
}
```

### Market Access Control
Markets implement role-based access for specific operations:
- **Public Operations**: Supply, borrow, withdraw, liquidate
- **No Admin Operations**: Markets have no administrative override capabilities

## Owner Configuration

### Current Setup
- **Registry Owner**: Single account
- **Security Consideration**: Single point of failure exists

## Upgrade Capabilities

### Registry Upgrades
Registry contract is **immutable** - no upgrade functions exist:
1. **No Contract Replacement**: Registry cannot be upgraded once deployed
2. **Add Market Versions**: Can deploy new market contract versions via [`add_version()`](https://github.com/Templar-Protocol/contracts/blob/dev/contract/registry/src/lib.rs)
3. **Remove Versions**: Can remove outdated market versions via [`remove_version()`](https://github.com/Templar-Protocol/contracts/blob/dev/contract/registry/src/lib.rs)

### Market Version Updates
- **Version Deployment**: New market versions can be added to registry
- **User Migration**: Users must manually migrate to new market versions
- **No Forced Migration**: Existing markets continue operating

## Emergency Procedures

### Market Issues
Since markets are immutable:
- **Bug Discovery**: Deploy patched version through registry
- **User Migration**: Users must manually migrate to new markets
- **Asset Safety**: Existing deposits remain in original markets

### Registry Issues
Since registry is immutable:
- **No Emergency Upgrade**: Registry cannot be upgraded or fixed
- **No Automated Backup**: Manual procedures required
- **Version Management**: Can only add/remove market versions, not fix registry itself

## Current Governance Model

### Owner-Based Control
- **No Governance Token**: Protocol operates without governance tokens
- **Single Owner**: All administrative decisions made by registry owner account
- **No Voting Mechanism**: No on-chain voting implemented

## Time Locks and Delays

### Current Implementation
- **No Timelocks**: Registry owner can execute version management immediately
- **No Mandatory Delays**: No enforced waiting periods for adding/removing versions
- **No Registry Upgrades**: Registry contract cannot be upgraded

## Transparency and Monitoring

### Public Visibility
- **Source Code**: All code publicly available on [GitHub](https://github.com/Templar-Protocol/contracts)
- **Deployment History**: Registry maintains deployment records via [`list_deployments()`](https://github.com/Templar-Protocol/contracts/blob/dev/contract/registry/src/lib.rs)
- **Audit Reports**: Available at [audits/](https://github.com/Templar-Protocol/contracts/tree/dev/audits)

### Monitoring Systems
- **Contract State**: Manual monitoring required
- **Upgrade Notifications**: Manual notifications required
- **Community Communication**: Manual announcements through official channels

## Risk Assessment

### Current Mitigation
- **Code Review**: Security reviews before deployment
- **Open Source**: Public code visibility allows community review at [GitHub](https://github.com/Templar-Protocol/contracts)
- **Testing**: Comprehensive test suite ([280+ tests](https://github.com/Templar-Protocol/contracts/tree/dev/contract/market/tests))
- **Immutable Contracts**: Both registry and market contracts cannot be modified once deployed
