# Templar x Mezo: BTC Prime Asset Integration

## Overview

Templar lending markets can accept **BTC on Mezo** (Prime Asset) as collateral. BTC is minted on Mezo at the consensus level, backed by institutional-grade qualified custody at Anchorage Digital Bank. **NEAR Chain Signatures** enables Templar to sign cross-chain transactions — routing stablecoin liquidity from any chain and deploying borrowed assets into yield vaults.

---

## 1. Borrowing Flow

A borrower deposits BTC on Mezo as collateral into a Templar lending market. The BTC is backed by segregated, bankruptcy-remote custody at Anchorage Digital Bank. NEAR Chain Signatures enables cross-chain delivery of borrowed stablecoins to any chain the borrower chooses.

```mermaid
graph TB
    classDef user fill:#f5f5f5,stroke:#888,color:#333,font-weight:bold
    classDef mezo fill:#cce5ff,stroke:#4a90d9,color:#333
    classDef templar fill:#d8d0e8,stroke:#7b68ae,color:#333
    classDef mpc fill:#d1ecf1,stroke:#5ba7b5,color:#333
    classDef stable fill:#d4edda,stroke:#5a9e6f,color:#333

    USER["Borrower"]:::user

    USER -- "1. Deposits BTC as collateral" --> BTC

    subgraph MEZO ["Mezo Network"]
        BTC["BTC (Prime Asset)"]:::mezo
        CUSTODY["Anchorage Digital Bank<br>Qualified Custody"]:::mezo
        BTC -.- CUSTODY
    end

    BTC -- "2. Collateral locked" --> MARKET

    subgraph TEMPLAR ["Templar Protocol"]
        MARKET["Lending Market"]:::templar
        SIG{{"NEAR Chain Signatures<br>MPC Network"}}:::mpc
        MARKET -.- SIG
    end

    MARKET -- "3. Stablecoins borrowed" --> STABLES

    subgraph OUT ["Stablecoins on Any Chain"]
        direction LR
        STABLES["USDT0 / USDC / rlUSD"]:::stable
    end

    STABLES -- "4. Delivered to borrower" --> USER

    style MEZO fill:#f8f9fa,stroke:#dee2e6,color:#333
    style TEMPLAR fill:#f8f9fa,stroke:#dee2e6,color:#333
    style OUT fill:#f8f9fa,stroke:#dee2e6,color:#333
```

---

## 2. Supply Flow

A supplier deposits stablecoins from any supported chain into Templar lending markets. NEAR Chain Signatures routes the liquidity cross-chain into Templar on Mezo. Suppliers earn yield from borrower interest payments.

```mermaid
graph TB
    classDef user fill:#f5f5f5,stroke:#888,color:#333,font-weight:bold
    classDef chain fill:#cce5ff,stroke:#4a90d9,color:#333
    classDef templar fill:#d8d0e8,stroke:#7b68ae,color:#333
    classDef mpc fill:#d1ecf1,stroke:#5ba7b5,color:#333
    classDef yield fill:#d4edda,stroke:#5a9e6f,color:#333

    USER["Supplier"]:::user

    USER -- "1. Supplies stablecoins" --> CHAINS

    subgraph SOURCE ["From Any Chain"]
        direction LR
        CHAINS["Flare / Ethereum / Solana / Plasma"]:::chain
    end

    CHAINS -- "2. Routed cross-chain" --> SIG

    subgraph TEMPLAR ["Templar Protocol on Mezo"]
        SIG{{"NEAR Chain Signatures<br>MPC Network"}}:::mpc
        MARKET["Lending Market"]:::templar
        SIG -- "Deposits into" --> MARKET
    end

    MARKET -- "3. Borrowers pay interest" --> YIELD
    YIELD["Yield Earned"]:::yield
    YIELD -- "4. Returns to supplier" --> USER

    style SOURCE fill:#f8f9fa,stroke:#dee2e6,color:#333
    style TEMPLAR fill:#f8f9fa,stroke:#dee2e6,color:#333
```

---

## 3. Yield Vault Deployment

A user deposits BTC on Mezo as collateral, borrows stablecoins, and deploys them into yield-generating vaults across multiple chains via NEAR Chain Signatures. If the vault yield exceeds the borrow cost, the user earns a net profit — making their BTC holdings productive.

```mermaid
graph TB
    classDef user fill:#f5f5f5,stroke:#888,color:#333,font-weight:bold
    classDef mezo fill:#cce5ff,stroke:#4a90d9,color:#333
    classDef templar fill:#d8d0e8,stroke:#7b68ae,color:#333
    classDef mpc fill:#d1ecf1,stroke:#5ba7b5,color:#333
    classDef vault fill:#dce8f0,stroke:#6b8fb3,color:#333
    classDef yield fill:#d4edda,stroke:#5a9e6f,color:#333

    USER["User"]:::user

    USER -- "1. Deposits BTC" --> BTC

    subgraph MEZO ["Mezo Network"]
        BTC["BTC (Prime Asset)"]:::mezo
        CUSTODY["Anchorage Custody"]:::mezo
        BTC -.- CUSTODY
    end

    BTC -- "2. Collateral locked" --> MARKET

    subgraph TEMPLAR ["Templar Protocol"]
        MARKET["Lending Market"]:::templar
    end

    MARKET -- "3. Borrows stablecoins" --> SIG

    SIG{{"NEAR Chain Signatures<br>MPC Network"}}:::mpc

    SIG -- "4. Deploys to vaults" --> VAULTS

    subgraph VAULTS ["Yield Vaults on Any Chain"]
        direction LR
        V1["Mezo DeFi"]:::vault
        V2["Ethereum DeFi"]:::vault
        V3["Solana DeFi"]:::vault
    end

    V1 --> PROFIT
    V2 --> PROFIT
    V3 --> PROFIT

    PROFIT["5. Net Return =<br>Vault APY minus Borrow Cost"]:::yield

    PROFIT -- "Yield to user" --> USER

    style MEZO fill:#f8f9fa,stroke:#dee2e6,color:#333
    style TEMPLAR fill:#f8f9fa,stroke:#dee2e6,color:#333
    style VAULTS fill:#f8f9fa,stroke:#dee2e6,color:#333
```

---

## 4. Full Architecture Overview

The complete picture: BTC on Mezo (backed by Anchorage qualified custody) serves as collateral in Templar lending markets. Stablecoin liquidity flows in from suppliers on any chain. Borrowers can take stables directly or deploy them into yield vaults. NEAR Chain Signatures powers all cross-chain operations.

```mermaid
graph LR
    classDef mezo fill:#cce5ff,stroke:#4a90d9,color:#333
    classDef templar fill:#d8d0e8,stroke:#7b68ae,color:#333
    classDef mpc fill:#d1ecf1,stroke:#5ba7b5,color:#333
    classDef chain fill:#dce8f0,stroke:#6b8fb3,color:#333
    classDef vault fill:#d4edda,stroke:#5a9e6f,color:#333

    subgraph COL ["BTC Collateral (Mezo)"]
        direction TB
        BTC["BTC Prime Asset"]:::mezo
        ANC["Anchorage Custody"]:::mezo
    end

    subgraph CORE ["Templar Protocol"]
        direction TB
        MKT["Lending Markets"]:::templar
        SIG{{"Chain Signatures"}}:::mpc
        MKT -.- SIG
    end

    subgraph SUP ["Stablecoin Supply"]
        direction TB
        S1["Flare"]:::chain
        S2["Ethereum"]:::chain
        S3["Solana"]:::chain
        S4["Plasma"]:::chain
    end

    subgraph YIELD ["Yield Vaults"]
        direction TB
        V1["Mezo DeFi"]:::vault
        V2["Ethereum DeFi"]:::vault
        V3["Solana DeFi"]:::vault
    end

    COL -- "Collateral" --> CORE
    SUP -- "Liquidity" --> CORE
    CORE -- "Deploy to vaults" --> YIELD

    style COL fill:#f8f9fa,stroke:#dee2e6,color:#333
    style CORE fill:#f8f9fa,stroke:#dee2e6,color:#333
    style SUP fill:#f8f9fa,stroke:#dee2e6,color:#333
    style YIELD fill:#f8f9fa,stroke:#dee2e6,color:#333
```
