# Frontend Security

Templar Protocol is committed to maintaining robust security practices across our frontend infrastructure to protect users and their assets. The application hosted at **app.templarfi.org** implements multiple layers of defense to ensure the integrity, availability, and trustworthiness of the interface through which users interact with Templar's smart contracts.

## Hosting & DDoS Protection

The Templar frontend is deployed on **Vercel's edge network**, which provides built-in distributed denial-of-service (DDoS) mitigation at the infrastructure level. Vercel's global CDN automatically absorbs and filters volumetric attacks, rate-limits abusive traffic, and ensures high availability across geographically distributed edge nodes. This architecture provides resilience against Layer 3, Layer 4, and Layer 7 attack vectors without requiring additional proxy configurations.

## DNS Security

The **templarfi.org** domain is managed with the following protections in place:

- **Registrar lock** is enabled to prevent unauthorized domain transfers or modifications.
- **DNSSEC** validation is supported through our DNS provider to ensure the authenticity of DNS responses and prevent cache poisoning attacks.
- **DNS records** are configured to resolve exclusively to Vercel's verified edge infrastructure, minimizing the risk of DNS hijacking or man-in-the-middle redirection.
- Access to DNS management is restricted to authorized personnel with multi-factor authentication (MFA) enforced on all accounts with domain-level permissions.

## Frontend Integrity & Modification Detection

Templar employs several practices to detect and prevent unauthorized modifications to the frontend application:

- **Immutable deployments**: Each deployment on Vercel produces an immutable, content-addressed build artifact. Previous deployments can be instantly promoted or rolled back, ensuring that any unauthorized change can be quickly identified and reversed.
- **Build verification**: The frontend is built from a version-controlled source repository with branch protection rules. All production deployments originate from reviewed and approved code changes.
- **Subresource Integrity (SRI)**: Where applicable, external resources loaded by the frontend use integrity hashes to ensure that third-party scripts and stylesheets have not been tampered with.
- **Content Security Policy (CSP)**: HTTP security headers are configured to restrict the sources from which scripts, styles, and other resources can be loaded, mitigating cross-site scripting (XSS) and code injection risks.

## Intrusion Detection & Monitoring

The Templar frontend leverages monitoring and alerting mechanisms to detect suspicious activity:

- **Vercel's built-in analytics and logging** provide visibility into traffic patterns, error rates, and deployment activity, enabling rapid detection of anomalous behavior.
- **Automated alerts** are configured for deployment failures, unusual traffic spikes, and error rate thresholds.
- **Access control**: Administrative access to the deployment platform and associated infrastructure accounts is restricted by role-based permissions and protected by multi-factor authentication.

## Client-Side Security Best Practices

The frontend application follows industry-standard security practices:

- **No private key handling**: The frontend never requests, stores, or transmits private keys. All transaction signing is delegated to the user's connected wallet.
- **Strict input validation**: All user inputs are validated and sanitized on the client side before any contract interaction to prevent injection attacks and malformed transaction data.
- **HTTPS enforced**: All connections to app.templarfi.org are served exclusively over TLS, with HTTP Strict Transport Security (HSTS) headers enforced to prevent protocol downgrade attacks.
- **Minimal third-party dependencies**: The frontend minimizes its dependency surface area, and all packages are reviewed and version-pinned to reduce supply chain risk.
- **Wallet interaction safety**: Transaction parameters are constructed transparently, enabling users to verify contract calls and parameters in their wallet interface before signing.

## Incident Response

In the event that a frontend compromise is detected or suspected, Templar's response protocol includes:

- Immediate rollback to the last known-good immutable deployment.
- Revocation and rotation of any compromised credentials or API keys.
- Communication to users via official channels (Twitter, Discord, and documentation site) advising them to verify the application URL and refrain from signing transactions until the all-clear is issued.
- Post-incident review and publication of findings where appropriate.

---

*For questions or to report a security concern related to the Templar frontend, please contact the team via our official Discord or reach out on Twitter at [@TemplarFi](https://twitter.com/TemplarFi).*
