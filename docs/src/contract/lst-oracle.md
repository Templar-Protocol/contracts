# LST Oracle

The LST oracle adapter enhances base oracle functionality by supporting a broader range of asset classes. The primary transformation supported by the LST oracle adapter is price normalization of a liquid staking token (LST).

Price normalization requires retrieving the price of the underlying asset and the conversion rate between the LST and the underlying asset and combining them to produce a price for the LST asset itself.

## Example: stNEAR Price Calculation

Examine the transformer specification:

```bash
near contract call-function as-read-only \
    lst.oracle.tmplr.near get_transformer \
    json-args '{"price_identifier":"c23cb2430c81d475fbd1c235324d4987f2dd01431bf7ab3e7b9d69b9f6701470"}' \
    network-config mainnet \
    now
```

Output:

```json
{
  "action": {
    "NormalizeNativeLstPrice": {
      "decimals": 24
    }
  },
  "call": {
    "account_id": "meta-pool.near",
    "args": "bnVsbA==",
    "gas": "3000000000000",
    "method_name": "get_st_near_price"
  },
  "price_id": "c415de8d2eba7db216527dff4b60e8f3a5311c740dadb233e13e12547e226750"
}
```

This specification describes the following flow:

1. Retrieve the NEAR price from the Pyth oracle (asset ID `c415de8d2eba7db216527dff4b60e8f3a5311c740dadb233e13e12547e226750`).
2. Retrieve the stNEAR redemption rate from the staking contract (`meta-pool.near->get_st_near_price({})`).
3. Calculate `price_stnear = price_near * redemption_rate / 10^24`.
