# Security Reporting

Templar Protocol takes security seriously and encourages responsible disclosure of security vulnerabilities.

All smart contracts are open-source and use reproducible builds for maximum transparency. For an overview of the protocol's security posture (audits, formal verification, oracle safeguards, monitoring, and operational controls), see the [Security](./security-overview.md) page.

## Security Contact

Report security vulnerabilities and other sensitive issues by email to [security@templarprotocol.com](mailto:security@templarprotocol.com).

This is the single channel for responsible disclosure. Templar previously ran a public bug bounty program; that program has ended, and reports should no longer be submitted through third-party bounty platforms. Good-faith reports of genuine vulnerabilities may be rewarded at Templar's discretion.

Please do not disclose vulnerabilities publicly (on GitHub issues, social media, or community channels) before Templar has had the opportunity to investigate and remediate.

## Responsible Disclosure

If you have discovered a security issue, please follow these steps:

1. **Report**: Send vulnerability details to [security@templarprotocol.com](mailto:security@templarprotocol.com).
2. **Investigation**: The security team will acknowledge the report and assess its severity and impact.
3. **Resolution**: A fix is developed, reviewed, and deployed. Because market contracts are immutable, a fix to a market may involve deploying a patched version through the registry and coordinating user migration; see [Protocol Governance](./governance.md#emergency-procedures).
4. **Public Disclosure**: Coordinated disclosure after the fix is in place.

Security reports should include:

- A clear description of the vulnerability and its impact.
- The affected contract(s), account ID(s), or component(s).
- Steps to reproduce the issue, ideally with a proof of concept.

## Security Alerts

Important security notices will be posted on the official Discord server, Telegram channel, and X (Twitter) account. Real-time protocol alerts are also published to the public [Templar alerts Telegram channel](https://t.me/+CcqXyt01lsljZmQx); see [Monitoring and Risk Management](./monitoring.md).

## Audit Information

Audit reports and the formal verification report are available in the [Templar audits folder](https://drive.google.com/drive/folders/14Q6iysMotto5fqpu6LRxjWBqeXElzkyF?usp=sharing). A summary of each engagement is on the [Security](./security-overview.md#audits-and-formal-verification) page.

The [audits directory in the contracts repository](https://github.com/Templar-Protocol/contracts/tree/dev/audits) contains auditor-facing notes and the known-issues register (findings acknowledged or fixed after a report was issued). Audits of the NEAR infrastructure Templar depends on (nearcore, NEAR Intents, Omnibridge, Chain Signatures) are in the [NEAR dependency audits folder](https://drive.google.com/drive/folders/1_6MPZLrWxLTWpCi5caYC2IWKP-8sW1uP?usp=sharing).
