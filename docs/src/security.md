# Security Reporting

Templar Protocol takes security seriously and encourages responsible disclosure of security vulnerabilities.

## Security Contact

For security vulnerabilities and sensitive issues:
- **Email**: [security@templarprotocol.com](mailto:security@templarprotocol.com)
- **Telegram**: [@peer2f00l](https://t.me/peer2f00l)

## Security Alerts

Important security notices will be posted on the official Discord server, Telegram channel, and X (Twitter) account as stated in the [SECURITY.md](../SECURITY.md) file.

## Audit Information

- **Current Status**: `<TBD>`

## Current Security Measures

### Development Security

#### Code Review Process
- **GitHub Pull Requests**: Standard GitHub review process
- **Code Quality Tools**: Automated formatting (`cargo fmt`) and linting (`cargo clippy`)
- **Static Analysis**: Clippy linting integrated into CI workflows

#### Testing Security
- **Comprehensive Test Suite**: 280 tests covering protocol functionality
- **Security-Related Testing**: Tests include security scenarios
- **Integration Testing**: Full protocol interaction testing

### Contract Security

#### Access Control
Registry contracts implement owner-based access control:
```rust
#[payable]
pub fn add_version(&mut self, version_key: String, code: Vec<u8>) {
    self.assert_owner();  // Only owner can add versions
    // ...
}
```

#### Market Immutability
- **Market Contracts**: Immutable once deployed - no admin functions
- **Protocol Rules**: All operations follow predetermined protocol rules
- **No Pause Mechanisms**: Markets operate autonomously

### Deployment Security

#### Reproducible Builds
- **Deterministic Compilation**: All contracts built reproducibly
- **Code Verification**: Hash verification against deployed bytecode
- **Open Source**: All code publicly available for review

## Responsible Disclosure

### Disclosure Process
1. **Report**: Send vulnerability details to security@templarprotocol.com
2. **Investigation**: Security team will assess the report
3. **Resolution**: Fix development and deployment
4. **Public Disclosure**: Coordinated disclosure after fix

### Report Guidelines
Please include:
- Clear description of the vulnerability
- Steps to reproduce the issue
- Potential impact assessment
- Suggested fixes (if available)

## Community Involvement

- **Open Source Review**: All code available for community audit
- **Security Discussions**: Technical security discussions welcomed
- **Vulnerability Reporting**: Clear process for security researchers documented
