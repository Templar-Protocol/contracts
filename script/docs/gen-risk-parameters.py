#!/usr/bin/env python3
"""Render the per-market risk-parameter tables in docs/src/risk-parameters.md.

Usage: gen-risk-parameters.py [--check | --stdout] [--all] [--self-test]

Reads the market specs under `deployments/v1/`, resolves each one's `extends`
chain the way `tmplrmgr` does (see `tools/manager/src/spec/extends.rs`), and
writes the tables between the `BEGIN GENERATED` / `END GENERATED` markers of
the page. Prose outside the markers is hand-written and left alone.

By default only the markets listed on app.templarfi.org are rendered. That set
is the `listed` array in `script/docs/listed-markets.toml`; every other spec
must be named in its `unlisted` array, and the script exits 1 if a spec is in
neither (or in both), so a new market cannot be forgotten. `--all` renders every
spec except the liquidation-test market, for local inspection. `--check`
regenerates in memory and exits 1 with a diff if the page is stale; `--stdout`
prints the block instead of writing.

The output is a pure function of the inputs: no timestamps, no commit hashes.
"""

import argparse
import difflib
import json
import sys
import tomllib
from decimal import ROUND_HALF_EVEN, Decimal
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPECS = ROOT / "deployments" / "v1"
PATCHES = ROOT / "deployments" / "patches"
PAGE = ROOT / "docs" / "src" / "risk-parameters.md"

BEGIN = "<!-- BEGIN GENERATED: risk-parameters (script/docs/gen-risk-parameters.py) -->"
END = "<!-- END GENERATED: risk-parameters -->"

# Which specs the page renders is data, not code: see LISTING.
LISTING = ROOT / "script" / "docs" / "listed-markets.toml"

GITHUB_TREE = "https://github.com/Templar-Protocol/contracts/tree/dev"
NEARBLOCKS = "https://nearblocks.io/address"


# --- spec resolution (port of tools/manager/src/spec/extends.rs) -------------


def merge_to_depth(base, overlay, depth):
    """Merge `overlay` into `base`, overlay winning, recursing `depth` levels.

    Below the depth limit values replace wholesale. The limit is load-bearing:
    `[market]` keys merge individually, but an externally-tagged enum such as
    `interest_rate_strategy = { Piecewise = {...} }`, a `sources` array, or a
    range stating only `minimum` must replace the profile's value as a unit.
    """
    if isinstance(base, dict) and isinstance(overlay, dict) and depth > 0:
        for key, value in overlay.items():
            if key in base:
                base[key] = merge_to_depth(base[key], value, depth - 1)
            else:
                base[key] = value
        return base
    return overlay


def resolve(path, visiting=None):
    """The merged TOML document for `path`, with its `extends` chain applied."""
    visiting = set() if visiting is None else visiting
    canonical = path.resolve()
    if canonical in visiting:
        raise SystemExit(f"`extends` cycle through {canonical}")
    visiting.add(canonical)
    with open(canonical, "rb") as f:
        value = tomllib.load(f)
    parents = [canonical.parent / p for p in value.pop("extends", [])]
    merged = {}
    for parent in parents:
        merged = merge_to_depth(merged, resolve(parent, visiting), 2)
    merged = merge_to_depth(merged, value, 2)
    visiting.remove(canonical)
    return merged


def extends_chain(path):
    """Profile names in the order they are applied, outermost first."""
    with open(path, "rb") as f:
        value = tomllib.load(f)
    chain = []
    for parent in value.get("extends", []):
        parent_path = (path.parent / parent).resolve()
        chain.extend(extends_chain(parent_path))
        chain.append(parent_path.stem)
    return chain


# --- formatting ---------------------------------------------------------------


def fmt_decimal(text, places=4):
    """`1.19999…` → `1.2`; `0.0888…9` → `0.0889`. Rounded, trailing zeros dropped."""
    value = Decimal(text).quantize(Decimal(1).scaleb(-places), rounding=ROUND_HALF_EVEN)
    value = value.normalize()
    return format(value, "f") if value != value.to_integral() else str(int(value))


def fmt_ratio(text):
    """An MCR as `1.25 (125%)`."""
    ratio = fmt_decimal(text)
    pct = fmt_decimal(str(Decimal(text) * 100), 2)
    return f"{ratio} ({pct}%)"


def fmt_pct(text):
    return f"{fmt_decimal(str(Decimal(text) * 100), 2)}%"


def fmt_curve(irs):
    """The annualized borrow rate at zero, optimal, and full usage.

    Piecewise: `r(u) = rate_1 * u + base` below `optimal`, then
    `r(u) = rate_2 * u + optimal * (rate_1 - rate_2) + base`.
    Linear: `r(u) = u * (top - base) + base`.
    """
    if "Piecewise" in irs:
        p = {k: Decimal(v) for k, v in irs["Piecewise"].items()}
        at_zero = p["base"]
        at_optimal = p["rate_1"] * p["optimal"] + p["base"]
        at_full = p["rate_2"] + p["optimal"] * (p["rate_1"] - p["rate_2"]) + p["base"]
        return (
            f"{fmt_pct(at_zero)} at 0% usage, {fmt_pct(at_optimal)} at {fmt_pct(p['optimal'])} usage "
            f"(kink), {fmt_pct(at_full)} at 100% usage"
        )
    if "Linear" in irs:
        p = irs["Linear"]
        return f"Linear from {fmt_pct(p['base'])} at 0% usage to {fmt_pct(p['top'])} at 100% usage"
    return f"`{json.dumps(irs, sort_keys=True)}`"


def fmt_fee(fee):
    if "Flat" in fee:
        return f"Flat {fee['Flat']}"
    if "Proportional" in fee:
        return f"Proportional {fmt_pct(fee['Proportional'])}"
    return f"`{json.dumps(fee, sort_keys=True)}`"


def fmt_time_based_fee(tbf):
    fee = fmt_fee(tbf["fee"])
    if fee == "Flat 0 atoms":
        return "None"
    return f"{fee} over {tbf['duration']} ({tbf['behavior']})"


def fmt_range(r):
    if r is None:
        return "unbounded"
    lo = r.get("minimum")
    hi = r.get("maximum")
    parts = []
    parts.append(f"min {lo}" if lo is not None else "no minimum")
    parts.append(f"max {hi}" if hi is not None else "no maximum")
    return ", ".join(parts)


def fmt_yield(yw):
    static = yw.get("static", {})
    total = yw["supply"] + sum(static.values())
    parts = [f"suppliers {yw['supply']}/{total}"]
    for account, weight in static.items():
        parts.append(f"`{account}` {weight}/{total}")
    return "; ".join(parts)


def fmt_duration(text):
    return text if text is not None else "none"


def short_hex(text, keep=8):
    return f"`{text[:keep]}…`" if len(text) > keep + 1 else f"`{text}`"


def fmt_source(src):
    kind = src["kind"]
    oracle = f"`{src['oracle']}`"
    weight = f"weight {src['weight']}"
    if kind == "lazer":
        return f"Pyth Lazer feed {src['feed_id']} via {oracle}, {weight}"
    if kind == "pyth":
        return f"Pyth (classic) {short_hex(src['price_id'])} via {oracle}, {weight}"
    if kind == "red_stone":
        return f"RedStone `{src['price_id']}` via {oracle}, {weight}"
    if kind == "lst":
        return f"LST transformer via {oracle}, {weight}"
    return f"{kind} via {oracle}, {weight}"


def fmt_sources(asset):
    sources = asset.get("sources")
    if not sources:
        # A direct market: the price identifier is read from the oracle as-is.
        pid = asset.get("price_id")
        return f"feed {short_hex(pid)} read directly" if pid else "n/a"
    lines = [fmt_source(s) for s in sources]
    quorum = f"aggregator `{asset.get('aggregator', '?')}`, min sources {asset.get('min_sources', '?')}"
    return "<br>".join(lines + [quorum])


SYMBOLS = {
    "dejaaa": "deJAAA",
    "dejtrsy": "deJTRSY",
    "solvbtc": "SolvBTC",
    "hemibtc": "hemiBTC",
    "stnear": "stNEAR",
    "linear": "LiNEAR",
}


def asset_label(name_part):
    """`ixlmusdc` → `USDC on Stellar (via NEAR Intents)`, from the naming convention."""
    chains = {"eth": "Ethereum", "xlm": "Stellar", "sol": "Solana"}
    part = name_part
    via = ""
    if part.startswith("i"):
        via = " (via NEAR Intents)"
        part = part[1:]
    chain = ""
    for prefix, chain_name in chains.items():
        if part.startswith(prefix) and len(part) > len(prefix):
            chain = f" on {chain_name}"
            part = part[len(prefix):]
            break
    return f"{SYMBOLS.get(part, part.upper())}{chain}{via}"


# --- loading ------------------------------------------------------------------


def listing_errors(spec_names, listed, unlisted):
    """Why `listed`/`unlisted` do not partition `spec_names`; empty when they do."""
    errors = []
    for label, names in (("listed", listed), ("unlisted", unlisted)):
        dupes = sorted({n for n in names if names.count(n) > 1})
        if dupes:
            errors.append(f"duplicated in `{label}`: {dupes}")
    both = sorted(set(listed) & set(unlisted))
    if both:
        errors.append(f"in both `listed` and `unlisted`: {both}")
    known = set(listed) | set(unlisted)
    no_spec = sorted(known - set(spec_names))
    if no_spec:
        errors.append(f"named but have no spec under deployments/v1/: {no_spec}")
    unclassified = sorted(set(spec_names) - known)
    if unclassified:
        errors.append(
            f"have a spec but are in neither list (add them to `listed` or `unlisted`): {unclassified}"
        )
    return errors


def load_listing(spec_names):
    with LISTING.open("rb") as fh:
        data = tomllib.load(fh)
    listed = list(data.get("listed", []))
    unlisted = list(data.get("unlisted", []))
    errors = listing_errors(spec_names, listed, unlisted)
    if errors:
        where = LISTING.relative_to(ROOT)
        raise SystemExit("\n".join([f"{where} does not classify every market:"] + [f"  - {e}" for e in errors]))
    return set(listed)


def load_markets(all_specs):
    paths = [p for p in sorted(SPECS.glob("*.toml")) if not p.stem.startswith("liqtest-")]
    listed = load_listing([p.stem for p in paths])
    markets = []
    for path in paths:
        name = path.stem
        if not all_specs and name not in listed:
            continue
        spec = resolve(path)
        registry = spec["registry"]
        account = f"{name}.{registry}"
        direct = spec.get("oracle", {}).get("direct")
        markets.append(
            {
                "name": name,
                "account": account,
                "registry": registry,
                "market": spec["market"],
                "collateral": spec["collateral"],
                "borrow": spec["borrow"],
                "direct": direct["account_id"] if direct else None,
                "patched": (PATCHES / account).is_dir(),
                "profiles": extends_chain(path),
            }
        )
    return sorted(markets, key=lambda m: m["name"])


# --- rendering ----------------------------------------------------------------


def market_link(m):
    return f"[`{m['account']}`]({NEARBLOCKS}/{m['account']})"


def table(headers, rows):
    out = ["| " + " | ".join(headers) + " |", "|" + "---|" * len(headers)]
    out.extend("| " + " | ".join(r) + " |" for r in rows)
    return "\n".join(out)


def generate(markets):
    blocks = [BEGIN, "", "<!-- Generated by script/docs/gen-risk-parameters.py from deployments/v1/*.toml. Do not edit by hand; run `just docs-generate`. -->", ""]

    blocks.append("### Collateralization and Liquidation\n")
    rows = []
    for m in markets:
        c, b = m["name"].split("-")[:2]
        mk = m["market"]
        rows.append(
            [
                market_link(m),
                asset_label(c),
                asset_label(b),
                fmt_ratio(mk["mcr_maintenance"]),
                fmt_ratio(mk["mcr_liquidation"]),
                fmt_pct(mk["maximum_usage_ratio"]),
                fmt_pct(mk["liquidation_maximum_spread"]),
                mk["price_maximum_age"],
            ]
        )
    blocks.append(
        table(
            [
                "Market",
                "Collateral",
                "Borrow",
                "MCR (maintenance)",
                "MCR (liquidation)",
                "Max usage ratio",
                "Max liquidation spread",
                "Max price age",
            ],
            rows,
        )
    )

    blocks.append("\n### Interest and Fees\n")
    rows = []
    for m in markets:
        mk = m["market"]
        rows.append(
            [
                market_link(m),
                fmt_curve(mk["interest_rate_strategy"]),
                fmt_fee(mk["origination_fee"]),
                fmt_time_based_fee(mk["supply_withdrawal_fee"]),
                fmt_yield(mk["yield_weights"]),
                fmt_range(mk.get("borrow_range")),
                fmt_range(mk.get("supply_range")),
                fmt_range(mk.get("supply_withdrawal_range")),
                fmt_duration(mk.get("borrow_maximum_duration_ms")),
                mk["time_chunk"],
            ]
        )
    blocks.append(
        table(
            [
                "Market",
                "Interest rate curve (annualized)",
                "Origination fee",
                "Supply withdrawal fee",
                "Yield split",
                "Borrow range",
                "Supply range",
                "Supply withdrawal range",
                "Max borrow duration",
                "Time chunk",
            ],
            rows,
        )
    )

    blocks.append("\n### Oracle Configuration\n")
    rows = []
    for m in markets:
        if m["direct"]:
            mode = f"Direct read of `{m['direct']}`"
            gov = "Owner of that oracle (`own_get_owner`)"
        else:
            mode = f"Proxy oracle `proxy-oracle-{m['name']}.{m['registry']}`"
            gov = f"`proxy-gov-{m['name']}.{m['registry']}`"
        patched = (
            f"[Yes]({GITHUB_TREE}/deployments/patches/{m['account']})" if m["patched"] else "No"
        )
        rows.append(
            [
                market_link(m),
                mode,
                fmt_sources(m["collateral"]),
                fmt_sources(m["borrow"]),
                gov,
                patched,
            ]
        )
    blocks.append(
        table(
            [
                "Market",
                "Oracle",
                "Collateral price sources",
                "Borrow price sources",
                "Oracle governance",
                "Storage patched",
            ],
            rows,
        )
    )

    blocks.append("\n### Asset Identifiers\n")
    rows = []
    for m in markets:
        rows.append(
            [
                market_link(m),
                f"<code>{m['collateral']['asset']}</code>",
                str(m["collateral"]["decimals"]),
                f"<code>{m['borrow']['asset']}</code>",
                str(m["borrow"]["decimals"]),
                ", ".join(f"`{p}`" for p in m["profiles"]),
            ]
        )
    blocks.append(
        table(
            ["Market", "Collateral asset", "Decimals", "Borrow asset", "Decimals", "Profiles applied"],
            rows,
        )
    )

    blocks.append("")
    blocks.append(END)
    return "\n".join(blocks) + "\n"


def splice(page_text, block):
    start = page_text.find(BEGIN)
    end = page_text.find(END)
    if start < 0 or end < 0 or end < start:
        raise SystemExit(f"{PAGE} is missing the generated-block markers")
    end += len(END) + 1  # include the trailing newline
    return page_text[:start] + block + page_text[end:]


# --- self-test ----------------------------------------------------------------


def self_test():
    """The merge semantics the tables depend on."""
    base = {"market": {"a": "1", "borrow_range": {"minimum": "1", "maximum": "9"},
                       "irs": {"Piecewise": {"base": "0"}}}}
    overlay = {"market": {"b": "2", "borrow_range": {"minimum": "5"},
                          "irs": {"Linear": {"base": "0", "top": "0"}}}}
    merged = merge_to_depth(base, overlay, 2)
    assert merged["market"]["a"] == "1" and merged["market"]["b"] == "2", "section keys merge"
    assert merged["market"]["borrow_range"] == {"minimum": "5"}, "a range replaces wholesale"
    assert merged["market"]["irs"] == {"Linear": {"base": "0", "top": "0"}}, "an enum replaces wholesale"
    assert fmt_decimal("1.19999999999999999999999999999999999999") == "1.2"
    assert fmt_decimal("0.08888888888888888888888888888888888889") == "0.0889"
    assert fmt_ratio("1.25") == "1.25 (125%)"
    assert fmt_pct("0.99") == "99%"
    assert asset_label("ixlmusdc") == "USDC on Stellar (via NEAR Intents)"
    assert asset_label("ibtc") == "BTC (via NEAR Intents)"
    assert asset_label("iethhemibtc") == "hemiBTC on Ethereum (via NEAR Intents)"
    assert asset_label("ixlmdejaaa") == "deJAAA on Stellar (via NEAR Intents)"
    assert fmt_curve({"Piecewise": {"base": "0", "optimal": "0.9", "rate_1": "0.08888888888888888888888888888888888889", "rate_2": "2.4"}}) == "0% at 0% usage, 8% at 90% usage (kink), 32% at 100% usage"
    assert fmt_curve({"Piecewise": {"base": "0.03", "optimal": "0.9", "rate_1": "0", "rate_2": "2"}}) == "3% at 0% usage, 3% at 90% usage (kink), 23% at 100% usage"
    # The listing file must partition the specs.
    assert listing_errors(["a", "b", "c"], ["a"], ["b", "c"]) == []
    errs = listing_errors(["a", "b", "c", "d"], ["a", "a", "x"], ["b", "a"])
    assert any("duplicated in `listed`: ['a']" in e for e in errs), errs
    assert any("in both" in e and "['a']" in e for e in errs), errs
    assert any("no spec" in e and "['x']" in e for e in errs), errs
    assert any("neither list" in e and "['c', 'd']" in e for e in errs), errs
    print("self-test ok")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="exit 1 if the page is stale")
    parser.add_argument("--stdout", action="store_true", help="print the block instead of writing")
    parser.add_argument("--all", action="store_true", help="render every non-liqtest spec")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0

    block = generate(load_markets(args.all))
    if args.stdout:
        sys.stdout.write(block)
        return 0

    current = PAGE.read_text()
    updated = splice(current, block)
    if args.check:
        if updated == current:
            return 0
        sys.stdout.writelines(
            difflib.unified_diff(
                current.splitlines(keepends=True),
                updated.splitlines(keepends=True),
                fromfile=str(PAGE.relative_to(ROOT)),
                tofile="generated",
            )
        )
        print(f"\n{PAGE.relative_to(ROOT)} is stale; run `just docs-generate`.", file=sys.stderr)
        return 1
    if updated != current:
        PAGE.write_text(updated)
        print(f"updated {PAGE.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
