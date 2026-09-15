# Security

Templar Protocol is built on a defence-in-depth model: immutable, isolated market contracts; multi-source oracles with circuit breakers; independent audits and formal verification; continuous monitoring with 24/7 human coverage; and timelocked, multisig-controlled governance for every component that can change. This page summarizes those measures for users, integrators, and liquidity providers performing due diligence. For vulnerability reporting, see [Security Reporting](./security.md).

## At a Glance

- **Five independent audits plus formal verification** by Guvenkaya, Thesis Defense, Certora, and Halborn (twice). All critical and high-severity findings remediated. [Reports](https://drive.google.com/drive/folders/14Q6iysMotto5fqpu6LRxjWBqeXElzkyF?usp=sharing).
- **Immutable market contracts**: no contract-level admin functions, upgrade method, pause function, or parameter mutators. Retained deployer keys on some market accounts are a separate, documented storage-patch exception (see [Immutable Markets](#immutable-markets)).
- **Isolated markets**: one collateral asset and one borrow asset per contract; no cross-asset contagion.
- **Proxy oracles** aggregating Pyth and RedStone with freshness filters and **circuit breakers**, the emergency brake for immutable markets.
- **Professional curators** manage vault lending risk under timelocked governance with an independent Sentinel.
- **Real-time monitoring** of markets, proxy oracles, and vaults with [Hypernative](https://www.hypernative.io/) and custom alerts, a [public alerts channel](https://t.me/+CcqXyt01lsljZmQx), and a [live risk dashboard](https://data.templarfi.org/).
- **AI security-focused code scanning** (Octane, Almanax, GPT Cyber) alongside nightly formal proofs and fuzzing in CI.
- **2-of-3 DAO multisig** for every mutable NEAR component; timelocks and immediate actions follow each component's policy (proxy oracle governance is timelocked per method, the registry uses two-step finalization, and breaker operation is immediate). Stellar vaults are governed per vault by a governance admin, curator, and Sentinel under configurable timelocks. See [Protocol Governance](./governance.md).
- **Hardened frontend and DNS**.

## Audits and Formal Verification

All audit reports and the formal verification report are available in the [Templar audits folder](https://drive.google.com/drive/folders/14Q6iysMotto5fqpu6LRxjWBqeXElzkyF?usp=sharing).

| Engagement | Auditor | Scope | Completed |
|---|---|---|---|
| [NEAR smart contract security review](https://drive.google.com/file/d/1pGTVIrifXed-4wLSgUggXyfx60UDbpNl/view?usp=sharing) | [Guvenkaya](https://www.guvenkaya.co/) | Market contracts and shared protocol logic | April 2025 |
| [Smart contracts security audit](https://drive.google.com/file/d/1WJOWNU5sb-yPPn6ppo69ZCYiI__dJZMF/view?usp=sharing) | [Thesis Defense](https://thesis.co/defense) | Market contracts and shared protocol logic | July 2025 |
| [Security assessment and formal verification](https://drive.google.com/file/d/1Rk5W26mSXtB8_6Ti4iLQc3ZikNG3c0SJ/view?usp=sharing) | [Certora](https://www.certora.com/) | Market contracts: manual audit plus formal verification of borrow-position health, collateralization, liquidation, and repayment integrity properties | September 2025 |
| [Curated vaults audit](https://drive.google.com/file/d/1fV46zN9bd7ZozAsDLZyc71Zvi240yeAY/view?usp=sharing) | [Halborn](https://www.halborn.com/) | Curated vault stack (kernel, Stellar runtime, governance, share token, adapters) | June 2026 |
| [Proxy oracle audit](https://drive.google.com/file/d/1KOVYEiz8pGcWRWJ_-NFDa94LJ_f2zMWw/view?usp=sharing) | [Halborn](https://www.halborn.com/) | Proxy oracle kernel, NEAR and Stellar runtimes, and governance | August 2026 |

- **Coverage of live code**: market contracts 100%; proxy oracle 100%; curated vault stack approximately 85% (the custodial adapter was out of scope).
- **Remediation**: every critical and high-severity finding across all engagements has been fully remediated. Medium findings are remediated within a release cycle or accepted with written rationale; the [known-issues register](https://github.com/Templar-Protocol/contracts/tree/dev/audits) records findings acknowledged after a report was issued.
- **Audit cadence**: every material contract release (new contract, new external function, change to accounting or authorization semantics) is audited before it is deployed to mainnet. Auditor selection deliberately rotates so that no single firm owns every code path.
- **Bytecode verification**: mainnet deployments use reproducible builds, and the on-chain code hash is checked against the audited commit before deployment. Anyone can verify a deployed contract; see [Contract Verification](./addresses.md#contract-verification).

Beyond the external engagements, verification runs continuously inside the repository:

- **Formal verification in CI**: [Kani](https://model-checking.github.io/kani/) model-checking proofs over the vault kernel (asset conservation across allocation, withdrawal, refresh, and emergency-recovery flows; fee accrual; withdrawal escrow) and the universal-account authentication and migration logic run nightly and on every relevant pull request ([workflow](https://github.com/Templar-Protocol/contracts/actions/workflows/kani.yml)).
- **Fuzzing**: twenty libFuzzer targets covering borrow, supply, liquidation, interest and fee math, decimal arithmetic, snapshots, market creation, the vault state machine, and Stellar storage codecs run nightly, with committed regression seeds ([fuzz targets](https://github.com/Templar-Protocol/contracts/tree/dev/fuzz)).
- **Independent process review**: DeFiSafety's [Process Quality Review of Templar](https://drive.google.com/file/d/1rXVCHI-7XjCcyPAuoO44Kbs1kfwmxXkv/view?usp=sharing) scored the protocol 94% (PASS).

### Dependency Audits

Templar's cross-chain flows rely on NEAR infrastructure that is audited independently of Templar: NEAR Intents (deposits and withdrawals), the Omnibridge, Chain Signatures (the MPC network), and nearcore. Those reports are collected in the [NEAR dependency audits folder](https://drive.google.com/drive/folders/1_6MPZLrWxLTWpCi5caYC2IWKP-8sW1uP?usp=sharing): nearcore assessments by Trail of Bits and Sigma Prime; NEAR Intents reviews by Hacken, Guvenkaya, zkSecurity, and Aurora Labs; Omnibridge audit reports; and the NEAR One reports on the MPC Chain Signatures network. Contingency procedures for a dependency incident are part of the [incident response](#incident-response) runbooks.

## Smart Contract Architecture

### Immutable Markets

Market contracts have **no administrative functions**: no owner, no upgrade method, no pause switch, and no method to modify collateralization ratios, interest rate curves, fees, or oracle configuration after launch. Nothing in the contract's code lets anyone change the configured market parameters a user relied on when opening a position. (Interest rates move with utilization along the fixed curve, and valuations follow the oracle; the curve and the oracle configuration themselves cannot change.)

One qualification applies at the account level rather than the contract level. A market account whose deployer full-access key has been removed cannot be changed by anyone. Where that key is still held (by the multisig that deployed the market), contract storage can be modified through the reviewed, sandbox-replayed patch process described in [Deploying a market](./deployments.md#patching-contract-storage); this is how legacy markets were migrated from direct Pyth reads to proxy oracles. Whether a market account holds any access keys is visible on-chain:

```bash
near account list-keys <market-address> network-config mainnet now
```

When a new market version is released, it is registered in the [registry](./contract/registry.md) and deployed to a new account; existing markets keep running unchanged and users migrate voluntarily. This removes the largest single risk class in DeFi lending, a compromised or misused admin key draining or re-parameterizing live markets through the contract itself, at the cost of making incident response rely on the oracle layer and on migration rather than on in-place upgrades. See [Protocol Governance](./governance.md).

### Isolated Markets

Each market pairs exactly one collateral asset with one borrow asset in its own contract account, with its own supply pool, interest rate model, and risk parameters. There is no shared liquidity, no cross-collateralization, and no protocol-wide bad-debt socialization. A collateral asset that depegs, an oracle feed that fails, or a liquidation cascade in one market cannot spread to suppliers in any other market. Riskier or newer assets are onboarded in their own markets with their own conservative parameters, rather than added to a shared pool.

### Circuit Breakers as the Emergency Brake

Because markets cannot be paused, Templar puts the emergency control where the risk enters: the price feed. Markets configured with a [proxy oracle](./oracles.md#proxy-oracle) (every new market, and the legacy markets that have been migrated) read feeds that carry [circuit breakers](./oracles.md#circuit-breakers). Automatic rules block sudden jumps, staged ramps, and cumulative drift, and an operator can trip a feed manually with no timelock. While a feed is blocked, borrowing, collateral withdrawal against debt, and liquidations on that market stop; supply withdrawals, repayments, and collateral withdrawals from debt-free positions continue. This gives the team an immediate, reversible containment action that cannot be used to seize funds or change market terms.

A small number of older markets still read Pyth's contract directly and have no trip mechanism; for those, the containment options are halting Templar's bots and migrating users to a replacement market. Which oracle a market reads is visible in its `get_configuration` output.

### Defensive Valuation and Conservative Parameters

- Collateral is valued at the lower bound of the oracle confidence interval and liabilities at the upper bound.
- Aggregated prices use a weighted median biased low for collateral and high for liabilities.
- Every market enforces a maintenance collateralization ratio above its liquidation ratio, a maximum usage ratio that preserves withdrawal liquidity, bounded liquidation spreads, and partial liquidation that restores health rather than closing positions entirely.
- Market parameters are deployed from reviewed, version-controlled specifications with automated preflight checks, including a cross-check of every oracle price against an independent reference before a market goes live. See [Deploying a market](./deployments.md).

### Reproducible Builds and On-Chain Verification

All contracts are open source and built reproducibly. The deployed code hash of any Templar contract can be verified against the tagged source commit with a single command; see [Contract Verification](./addresses.md#contract-verification). Released contract artifacts are pinned by SHA-256 in the repository and are immutable once published.

## Oracle Security

Oracle failure and manipulation is the dominant cause of lending-protocol losses. Templar's [proxy oracles](./oracles.md#proxy-oracle) aggregate multiple independent oracles for redundancy and gate every price through freshness filters and circuit breakers:

- **Multiple sources**: Pyth (Lazer and classic) and RedStone are live; Chainlink and Atlas are being added. Sources, weights, and the fresh-source quorum are configured per feed.
- **Freshness filters** drop stale and future-dated prices before aggregation.
- **Weighted-median aggregation** across sources with a configurable fresh-source quorum. The standard production configuration uses a quorum of one with Pyth weighted above RedStone, so the higher-weighted source determines the price while both are fresh and a single fresh source carries the feed when the other is stale. That configuration favours liveness; the freshness filter and enforced circuit breakers, not the aggregation, are the primary defence against a single compromised provider, and quorum and weights can be raised per feed as more providers come online.
- **Circuit breakers** compare against accepted history, so an attacker cannot first poison the reference sample and then pass a deviation check.
- **Fail-closed reads**: a blocked, failed, or stale feed reads as no price, never as the last known price.
- **Timelocked governance** on every feed configuration change and contract upgrade. Circuit breaker operation (trip and untrip, re-arm, enforcement on or off) is role-gated and immediate in both directions; those roles sit with the multisig and every use is alerted.
- **Independently audited** by Halborn (August 2026).

Source: [`contract/proxy-oracle`](https://github.com/Templar-Protocol/contracts/tree/dev/contract/proxy-oracle).

## Curated Vaults and Professional Curators

Templar's curated vaults (currently on Stellar) let depositors hold a single vault share while professional curators manage lending risk on their behalf. Curators decide which markets a vault may allocate to, set per-market and per-group caps, and configure fees, under a governance model designed so that no single role can both take risk and escape oversight:

- **Timelocked governance**: risk-increasing actions (cap increases, new markets, fee increases, unpause, role changes) must mature under a per-action timelock before they execute. Risk-reducing actions (cap decreases, fee decreases) execute immediately.
- **Independent Sentinel**: a separate emergency role can pause the vault and tighten restrictions instantly, but cannot unpause, relax restrictions, or accept proposals.
- **Bounded exposure**: absolute and relative caps per market and per correlated group.
- **Audited and formally verified**: the vault stack was audited by Halborn, and the kernel's accounting invariants are proven with Kani in CI.
- **Continuously monitored** by Hypernative for exploit patterns, invariant violations, and anomalous privileged calls.

See the [Stellar Vault Curator Guide](./curator-guide.md) for the full operating model.

## Monitoring, Alerting, and Risk Dashboard

- **Live risk dashboard**: [data.templarfi.org](https://data.templarfi.org/) shows, in real time and per market, collateral coverage, liquidation proximity with drawdown scenarios, oracle health per feed, TVL and revenue, utilization and rates, borrower and supplier concentration, supply-side flows, positions by risk tier, a ledger of events, and post-withdrawal transfers through NEAR Intents, so anyone can assess the protocol's risk without relying on the team's reporting.
- **Alerting**: [Hypernative](https://www.hypernative.io/) monitors the Stellar vaults for exploit detection, invariant checks, and privileged-call anomalies. Custom alerts cover the NEAR markets and proxy oracles. Together the alerting covers oracle failure or price deviation beyond threshold, positions approaching liquidation, liquidations executed, large position events (whale alerts), bad debt creation, utilization crossing critical thresholds, and smart contract pauses or emergency admin actions. Alerts are published to the public [Templar alerts Telegram channel](https://t.me/+CcqXyt01lsljZmQx).
- **24/7 coverage**: Templar's written security policy defines an on-call rotation among the multisig signers, who are distributed across multiple time zones, so that a responder is within working hours at all times and alerts page the current on-call directly.
- **Compliance screening**: on-chain sanctions and risk signals from [Predicate](https://predicate.io/) and [TRM Labs](https://www.trmlabs.com/) flag SEVERE-labelled accounts interacting with Templar contracts.

Details, including how to run the same health checks yourself, are on the [Monitoring and Risk Management](./monitoring.md) page.

## Secure Development and AI-Assisted Code Scanning

- **Review and CI**: every change to protocol code lands through a pull request with required approving reviews, signed commits, and a passing CI gate (unit, integration, and sandbox node tests, lint, formatting, coverage, and dependency vulnerability scanning). Toolchains and dependencies are pinned; new dependencies require a second reviewer and a written rationale.
- **AI security-focused code scans**: in addition to human review and external audits, Templar runs AI-driven security analysis over the contracts using Octane, Almanax, and GPT Cyber (OpenAI's closed-access cybersecurity model). These scans are used to surface candidate issues for human triage; they complement rather than replace audits.
- **Formal proofs and fuzzing in CI**: see [Audits and Formal Verification](#audits-and-formal-verification).
- **Threat models**: components ship with written threat models and audit boundaries (for example the [Stellar vault STRIDE model](https://github.com/Templar-Protocol/contracts/blob/dev/contract/vault/soroban/STRIDE.md) and the [proxy oracle audit boundary](https://github.com/Templar-Protocol/contracts/blob/dev/contract/proxy-oracle/soroban/AUDIT.md)).
- **No hotfix bypass**: even during an incident, contract code changes go through the full audit-and-multisig path. The incident toolkit (circuit breakers, Sentinel pauses, cap-to-zero, halting bots) exists to buy time so that fixes are never rushed.
- **Deployment discipline**: mainnet deployments are planned from declarative specifications, reviewed as a plan artifact, preflighted against chain state, and applied with the multisig; see [Deploying a market](./deployments.md). Storage patches to live contracts are replayed in a sandbox and stamped before they can be applied.

## Operational Security and Key Management

- **Multisig control**: every mutable Templar contract on NEAR (the registry, proxy oracle governance, and adapters) is administered by [`templar.sputnik-dao.near`](https://nearblocks.io/address/templar.sputnik-dao.near), a Sputnik DAO with three signers on its council at a 2-of-3 threshold. Adding or removing a signer is itself a governed DAO proposal.
- **Timelocks**: proxy oracle governance typically applies 24-hour to 168-hour timelocks to configuration and upgrades (the exact policy is set per contract), and vault governance applies configurable per-action timelocks. Circuit breaker operation (trip and untrip, re-arm, enforcement on or off) is immediate in both directions, so those operator roles are held as tightly as the admin role and every use is alerted. See [Protocol Governance](./governance.md) for the full table.
- **Least privilege**: proxy oracle governance separates the `ManualTripper`, `CircuitBreakerOperator`, `ProxyConfigurationManager`, and `Admin` roles so that the account able to hit the emergency brake need not be able to reconfigure feeds or upgrade code.
- **Key hygiene**: privileged keys are held on hardware devices with geographically distributed cold backups; infrastructure and vendor accounts require MFA and have at least two administrators for continuity.
- **Security policy**: Templar maintains a written organizational security policy (access control, key management, SDLC, monitoring, incident response, vendor risk, business continuity) reviewed quarterly and shared with counterparties on request.

## Incident Response

Templar maintains role-segmented [emergency runbooks](https://github.com/Templar-Protocol/blend-contracts-v2/tree/main/docs/emergency-runbooks) covering markets, vaults, oracles, NEAR Intents, bridges, and stablecoin issuers. The process from detection to resolution:

1. **Detect and page**: a Hypernative or custom alert reaches the on-call responder.
2. **War room**: the responder opens a war room, names an incident lead, communications lead, and scribe, and classifies severity.
3. **Contain with the smallest reversible action**: halt Templar's own bots; trip the relevant proxy oracle feed to freeze price-dependent operations on markets that read a proxy oracle; on vaults, Sentinel pause or restriction tightening, allocator abort, and curator cap-to-zero, in that order of escalation.
4. **Preserve user exits**: no role can disable supply withdrawal requests or repayments at the market boundary.
5. **Recover**: a patched market version is audited, registered, and deployed through the registry; users migrate. There is no in-place hotfix.
6. **Stand down and learn**: stand-down requires sign-off from at least two roles; every user-affecting incident produces a post-mortem, published where appropriate.

Templar coordinates with the NEAR Foundation and the Stellar Development Foundation security functions during ecosystem-level incidents.

## Insurance and Recovery

Templar does not currently carry insurance cover for smart contract or oracle failure, and there is no on-chain insurance fund that backstops bad debt; options are under evaluation (**Input needed**: the options being evaluated and the decision timeline). Users with assets or positions in an exploited contract should assume that unrecovered exploit losses are borne by the users whose assets or positions are affected, including borrowers whose collateral is lost. Unrecoverable bad debt is borne by the affected market's suppliers and may reduce the share value of vaults allocated to that market. Markets are isolated, so neither kind of loss spreads to other markets.

Recovery from a protocol-level incident follows the [incident response](#incident-response) process: contain through the oracle layer and the bots, preserve user exits, and migrate to a patched, audited market version. For incidents at the infrastructure layer (NEAR Intents, bridges, stablecoin issuers), Templar coordinates with the NEAR Foundation and the Stellar Development Foundation security functions, following the recovery precedents those ecosystems have established. **Input needed**: whether any treasury-backed remediation policy applies, and its limits.

## Frontend Security

The application at **app.templarfi.org** is the interface most users sign transactions through, so it is hardened as a first-class attack surface.

### Hosting and DDoS Protection

The frontend is deployed on Vercel's edge network, which provides distributed denial-of-service mitigation at the infrastructure level. The global CDN absorbs and filters volumetric attacks, rate-limits abusive traffic, and serves the application from geographically distributed edge nodes, providing resilience against Layer 3, 4, and 7 attacks.

### DNS Hardening

The **templarfi.org** domain is protected by:

- **Registrar lock** to prevent unauthorized transfers or modifications.
- **DNSSEC** validation to ensure the authenticity of DNS responses and prevent cache poisoning.
- **Pinned records**: DNS resolves exclusively to Vercel's verified edge infrastructure, minimizing the risk of hijacking or redirection.
- **Restricted access**: DNS and registrar management is limited to authorized personnel with multi-factor authentication enforced on every account with domain-level permissions.

### Integrity and Modification Detection

- **Immutable deployments**: every deployment produces an immutable, content-addressed build; any unauthorized change can be identified and rolled back instantly.
- **Build verification**: production deployments originate only from reviewed and approved changes in a branch-protected repository.
- **No third-party scripts**: the application loads no scripts or stylesheets from external origins. Everything is bundled into content-hashed assets from the immutable build, so there is no external resource whose integrity would need to be pinned.
- **Framing protection**: every response carries `X-Frame-Options: SAMEORIGIN` and a Content Security Policy of `frame-ancestors 'none'`, so the application cannot be embedded in another site to trick users into signing (clickjacking). The policy does not currently restrict script or style sources; the absence of third-party scripts limits the injection surface such a restriction would cover. A policy restricting `script-src` and `style-src` to the application's own origin is a planned hardening (**Input needed**: owner and timing; the frontend is maintained in a separate repository).

### Intrusion Detection and Monitoring

- Platform analytics and logging provide visibility into traffic patterns, error rates, and deployment activity.
- Automated alerts fire on deployment failures, unusual traffic spikes, and error-rate thresholds.
- Administrative access to the deployment platform and related accounts is role-based and protected by multi-factor authentication.

### Client-Side Practices

- **No private key handling**: the frontend never requests, stores, or transmits private keys; signing is delegated to the user's wallet.
- **Strict input validation** before any contract interaction.
- **HTTPS enforced**: all traffic is served over TLS and plain HTTP is redirected to HTTPS. HTTP Strict Transport Security is a deployment requirement satisfied by the hosting platform's configuration rather than by headers set in the application code.
- **Minimal, pinned dependencies** to reduce supply-chain risk.
- **Transparent transactions**: parameters are constructed so users can verify the contract call and arguments in their wallet before signing.

### Frontend Incident Response

If a frontend compromise is detected or suspected: immediate rollback to the last known-good deployment; revocation and rotation of any affected credentials; communication on official channels advising users to verify the application URL and refrain from signing until the all-clear; and a post-incident review.

## Responsible Disclosure

Report vulnerabilities to [security@templarprotocol.com](mailto:security@templarprotocol.com). See [Security Reporting](./security.md) for the disclosure process.
