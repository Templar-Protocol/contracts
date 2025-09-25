# Protocol Governance and Administrative Controls

This document outlines the current administrative structure and governance controls of Templar Protocol.

## Contract Upgradeability

### Registry Contract
- **Upgrade Status**: Upgradeable through owner account
- **Owner Account**: `<TBD>`
- **Upgrade Mechanism**: Direct contract code replacement
- **Timelock**: No timelock implemented

### Market Contracts
- **Upgrade Status**: Immutable once deployed
- **Deployment**: Markets are deployed through the registry with immutable code
- **Version Control**: New market versions can be deployed through registry
- **Existing Markets**: Cannot be upgraded; users must migrate to new versions

## Administrative Roles and Capabilities

### Registry Owner Capabilities
The registry owner account has the following administrative privileges:

1. **Version Management**
   - Add new market contract versions: `add_version()`
   - Remove contract versions: `remove_version()`
   - Deploy new markets: `deploy_market()`

2. **Market Deployment**
   - Deploy markets with specific configurations
   - Set initial full-access keys for deployed markets
   - Control market naming and sub-account creation

3. **Registry Management**
   - Update registry contract code
   - Manage storage and versioning

### Market Contract Administration
Once deployed, market contracts have **no admin functions**:
- Markets operate autonomously based on initial configuration
- No ability to pause, upgrade, or modify market parameters
- All operations follow predetermined protocol rules

## Access Control Implementation

### Registry Access Control
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
Registry owner can:
1. **Replace Contract Code**: Direct contract upgrade capability
2. **Add Market Versions**: Deploy new market contract versions
3. **Remove Versions**: Remove outdated market versions

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
- **Emergency Upgrade**: Owner can deploy fixes immediately
- **No Automated Backup**: Manual procedures required

## Current Governance Model

### Owner-Based Control
- **No Governance Token**: Protocol operates without governance tokens
- **Single Owner**: All administrative decisions made by registry owner account
- **No Voting Mechanism**: No on-chain voting implemented

## Time Locks and Delays

### Current Implementation
- **No Timelocks**: Registry owner can execute upgrades immediately
- **No Mandatory Delays**: No enforced waiting periods for changes

## Transparency and Monitoring

### Public Visibility
- **Source Code**: All code publicly available on GitHub
- **Deployment History**: Registry maintains deployment records via `list_deployments()`
- **Audit Reports**: `<TBD>`

### Monitoring Systems
- **Contract State**: Manual monitoring required
- **Upgrade Notifications**: Manual notifications required
- **Community Communication**: Manual announcements through official channels

## Risk Assessment

### Current Mitigation
- **Code Review**: Security reviews before deployment
- **Open Source**: Public code visibility allows community review
- **Testing**: Comprehensive test suite (280+ tests)
- **Immutable Markets**: Market contracts cannot be modified once deployed
