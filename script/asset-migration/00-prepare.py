#!/usr/bin/env python3
"""Inspect a scoped campaign and freeze exact local execution intent; never submit."""
import base64
import hashlib
import json
from pathlib import Path

from common import asset_reference, canonical, canonical_dao_policy, normalize_campaign, registration_funded, require, stage_main
from governance import context_from_reads



def relative(run, path):
    return str(Path(path).relative_to(run.root))


def token_observation(run, asset, account):
    reference = asset_reference(asset)
    metadata = run.read("token.getMetadata", {"token": reference})
    require(isinstance(metadata, dict) and type(metadata.get("decimals")) is int, "token decimals are unavailable")
    return {"reference": reference, "metadata": metadata, "balance": str(run.balance(reference, account))}


def registry_scope(run):
    discovered = {}
    for registry in run.campaign["registry_accounts"]:
        accounts, offset = [], 0
        while True:
            page = run.read("registry.listDeploymentsByKind", {
                "registry_id": registry, "kind": "market", "offset": offset, "count": 100,
            })
            require(isinstance(page, list) and len(page) == len(set(page)), "ambiguous registry deployment page")
            accounts.extend(page)
            if len(page) < 100:
                break
            offset += len(page)
        require(len(accounts) == len(set(accounts)), "registry pagination returned duplicate accounts")
        discovered[registry] = accounts
    return discovered


def feed_links(run, configuration):
    price = configuration["price_oracle_configuration"]
    oracle = price["account_id"]
    require(run.read("contract.getKind", {"contract_id": oracle}) == "proxy_oracle", "migration requires a real proxy oracle")
    governance = run.read("owner.getOwner", {"contract_id": oracle})
    require(isinstance(governance, str) and governance, "proxy has no governance owner")
    target = run.read("proxyOracleGovernance.getProxyOracleId", {"governance_id": governance})
    policy = run.read("proxyOracleGovernance.getGovernancePolicy", {"governance_id": governance})
    roles = run.read("proxyOracleGovernance.getRoles", {"governance_id": governance, "account_id": run.campaign["dao"]})
    result = []
    for feed in sorted({price["borrow_asset_price_id"], price["collateral_asset_price_id"]}):
        proxy = run.read("proxyOracle.getProxy", {"oracle_id": oracle, "id": feed})
        breaker = run.read("proxyOracle.getProxyCircuitBreakerSet", {"oracle_id": oracle, "id": feed})
        feed_identity = {"oracle": oracle, "feed": feed, "governance": governance}
        live = context_from_reads(feed_identity, [governance, target, policy, roles, breaker, proxy])
        result.append({**feed_identity,
                       "initial_breaker": live["breaker"], "initial_manual": live["breaker"]["is_manually_tripped"],
                       "initial_proxy": live["proxy"], "governance_policy": live["policy"], "governance_roles": live["roles"]})
    return result


def handle(run):
    if run.intent:
        if run.args.campaign is not None:
            require(normalize_campaign(run.args) == run.campaign, "authored campaign changed after intent freeze")
        evidence = run.recheck_progress()
        prepared = {"intent_digest": run.intent["intent_digest"], "already_prepared": True, "current": evidence}
        if run.confirmed("00-prepare", "stage") is None:
            run.confirm("00-prepare", "stage", prepared)
        return prepared
    campaign = normalize_campaign(run.args)
    run.configure(campaign)
    identity = run.read("chain.getIdentity", None)
    require(identity == {"chain_id": campaign["expected_chain_id"], "genesis_hash": campaign["expected_genesis_hash"]},
            "wrong chain identity")
    policy = canonical_dao_policy(run.read("dao.getPolicy", {"dao_id": campaign["dao"]}))
    code = base64.b64decode(run.read("account.getCode", {"account_id": campaign["converter_account"]}), validate=True)
    require(hashlib.sha256(code).hexdigest() == campaign["converter_code_sha256"], "converter code identity mismatch")
    profile = run.read("contract.viewFunction", {"contract_id": campaign["converter_account"],
                       "method_name": "get_profile", "args": {"encoding": "json", "value": {}}})
    require(profile.get("schema") == 1 and profile.get("profile") == "local-one-to-one" and
            profile.get("settlement_rule") == "one-to-one-raw" and profile.get("requires_registration") is False,
            "unsupported local converter profile")
    markets, feeds, registrations = [], {}, {}
    for authored in campaign["markets"]:
        account = authored["account"]
        directory = run.path("00-prepare/markets/" + hashlib.sha256(account.encode()).hexdigest()[:24])
        directory.mkdir(parents=True, exist_ok=True)
        configuration = run.read("market.getConfiguration", {"market_id": account})
        changes = []
        for role in ("borrow", "collateral"):
            field = f"{role}_asset"
            if field in authored:
                require(configuration[field] != authored[field], f"unchanged requested {role} asset")
                changes.extend([{"op": "test", "path": f"/{field}", "value": configuration[field]},
                                {"op": "replace", "path": f"/{field}", "value": authored[field]}])
        changes_path = run.save(relative(run, directory / "changes.json"), changes)
        spec = directory / "config-patch.toml"
        if not spec.exists():
            run.command(["market", "generate-patch", "config", account, "--changes", str(changes_path), "--out", str(spec)])
        blobs = directory / "config-patch.blobs"
        before = json.loads((blobs / "before.json").read_text())
        after = json.loads((blobs / "after.json").read_text())
        require(before == configuration, "configuration changed during preparation")
        _, snapshot = run.snapshot(account, relative(run, directory / "initial"))
        path = authored.get("mpc_path", f"{campaign['dao']}-{account}")
        derived = run.command(["dao", "mpc", "derive-key", "--dao", campaign["dao"], "--mpc-contract", campaign["mpc_contract"],
                               "--key-type", campaign["mpc_key_type"], "--account-id", account, "--path", path])
        key = run.read("account.getAccessKey", {"account_id": account, "public_key": derived["public_key"]})
        require(key["permission"] == "FullAccess", "derived MPC key is not installed as full-access")
        access_keys = run.read("account.listAccessKeys", {"account_id": account})
        require(any(entry["public_key"] == derived["public_key"] and entry["nonce"] == key["nonce"]
                    and entry["permission"] == key["permission"] for entry in access_keys),
                "market access keys changed during preparation")
        roles, inventories, unchanged_assets = [], {}, []
        for role in ("borrow", "collateral"):
            field = f"{role}_asset"
            if field not in authored:
                unchanged_assets.append({"role": role, **token_observation(run, configuration[field], account)})
                continue
            old, new = configuration[field], authored[field]
            require(any(mapping == {"old_asset": old, "new_asset": new} for mapping in campaign["conversions"]),
                    "requested route is not explicitly mapped")
            routes = [route for route in profile["routes"] if route["old_asset"] == old and route["new_asset"] == new]
            require(len(routes) == 1, "converter has no unique confirmed route")
            old_info, new_info = token_observation(run, old, account), token_observation(run, new, account)
            decimals = configuration["price_oracle_configuration"][f"{role}_asset_decimals"]
            require(old_info["metadata"]["decimals"] == new_info["metadata"]["decimals"] == decimals,
                    "raw-unit decimals/rate changes cannot be handled by configuration replacement")
            source_key = canonical(old_info["reference"])
            if source_key in inventories:
                require(inventories[source_key]["new_asset"] == new, "incompatible duplicate source inventory")
                inventory_role = inventories[source_key]["inventory_role"]
            else:
                inventory_role = role
            record = {"role": role, "inventory_role": inventory_role, "old_asset": old, "new_asset": new,
                      "old_reference": old_info["reference"], "new_reference": new_info["reference"],
                      "initial_old_balance": old_info["balance"], "initial_new_balance": new_info["balance"],
                      "old_metadata": old_info["metadata"], "new_metadata": new_info["metadata"], "route_id": routes[0]["id"]}
            roles.append(record)
            inventories[source_key] = record
            for reference, beneficiary in ((new_info["reference"], account), (old_info["reference"], campaign["converter_account"])):
                contract = reference["contract_id"]
                bounds = run.read("storage.getBalanceBounds", {"contract_id": contract})
                balance = run.read("storage.getBalanceOf", {"contract_id": contract, "account_id": beneficiary})
                registrations[f"{contract}:{beneficiary}"] = {"contract_id": contract, "account_id": beneficiary,
                    "minimum": bounds["min"], "initial_balance": balance}
        for first in roles:
            for second in roles:
                require(first["old_reference"] != second["new_reference"], "cross-inventory input/output route is ambiguous")
        for feed in feed_links(run, configuration):
            feed_id = (feed["oracle"], feed["feed"])
            require(feed_id not in feeds or feeds[feed_id] == feed, "inconsistent shared-feed context")
            feeds[feed_id] = feed
        run.seal_tree(directory)
        markets.append({"account": account, "mpc_path": path, "public_key": derived["public_key"],
                        "initial_nonce": key["nonce"], "access_keys": access_keys,
                        "configuration": configuration, "expected_configuration": after,
                        "account_identity": run.identity(account), "snapshot": snapshot,
                        "changes": relative(run, changes_path), "layout_report": relative(run, blobs / "report.json"),
                        "roles": roles, "unchanged_assets": unchanged_assets})
    discovered = registry_scope(run)
    explicit = set(campaign["dependency_scope_accounts"]) | set(campaign["external_consumers"]) | {m["account"] for m in markets}
    candidates = explicit | {account for accounts in discovered.values() for account in accounts}
    dependencies = []
    selected = {market["account"] for market in markets}
    for account in sorted(candidates - selected):
        config = run.read("market.getConfiguration", {"market_id": account})
        prices = config["price_oracle_configuration"]
        affected = any((prices["account_id"], feed) in feeds for feed in
                       (prices["borrow_asset_price_id"], prices["collateral_asset_price_id"]))
        if not affected:
            continue
        require(account in explicit, "discovered shared-feed consumer is not in the operator-reviewed scope")
        _, snapshot = run.snapshot(account, "00-prepare/dependencies/" + hashlib.sha256(account.encode()).hexdigest()[:24])
        dependencies.append({"account": account, "configuration": config, "account_identity": run.identity(account),
                             "snapshot": snapshot, "assets": [token_observation(run, config[f"{role}_asset"], account)
                                                              for role in ("borrow", "collateral")]})
    for registration in registrations.values():
        if not registration_funded(registration, registration["initial_balance"]):
            directory = run.path("00-prepare/registration-preflight")
            directory.mkdir(parents=True, exist_ok=True)
            plan = run.plan("storage.ensureDeposit", {"contract_id": registration["contract_id"],
                "account_id": registration["account_id"], "mode": {"mode": "minimum_total", "amount": registration["minimum"]}}, campaign["dao"],
                directory / (hashlib.sha256(canonical(registration)).hexdigest() + ".json"))
            run.command(["dao", "propose", "--dao", campaign["dao"], "--plan", str(plan), "--description", "migration admission",
                         "--signer-id", campaign["submitter"], "--print", "json"])
    first_feed = next(iter(feeds.values()))
    policy_plan = run.plan("proxyOracle.updatePrices", {"oracle_id": first_feed["oracle"], "price_ids": [first_feed["feed"]]},
                           campaign["dao"], run.path("00-prepare/policy-preflight.plan.json"))
    run.command(["dao", "propose", "--dao", campaign["dao"], "--plan", str(policy_plan), "--description", "migration admission",
                 "--signer-id", campaign["submitter"], "--print", "json"])
    block = run.read("chain.getBlock", {})
    gas_budget = 2 * int(block["gas_price"]) * 300_000_000_000_000
    bond = int(policy["proposal_bond"])
    require(int(run.read("account.getSpendableBalance", {"account_id": campaign["submitter"]})) >= bond + gas_budget,
            "submitter cannot fund proposal bond and conservative live gas budget")
    if campaign["mpc_transport"] == "delegate-action":
        require(int(run.read("account.getSpendableBalance", {"account_id": campaign["relayer"]})) >= gas_budget,
                "relayer cannot fund the chosen transport")
    deposits = sum(max(0, int(item["minimum"]) - int((item["initial_balance"] or {"total": "0"})["total"]))
                   for item in registrations.values())
    require(int(run.read("account.getSpendableBalance", {"account_id": campaign["dao"]})) >= deposits + gas_budget,
            "DAO cannot fund required storage registrations")
    oracle_snapshots = {}
    for oracle in sorted({feed["oracle"] for feed in feeds.values()}):
        _, snapshot = run.snapshot(oracle, "00-prepare/oracles/" + hashlib.sha256(oracle.encode()).hexdigest()[:24])
        oracle_snapshots[oracle] = snapshot
    run.seal_tree(run.path("00-prepare"))
    intent = run.freeze({"schema": 1, "campaign": campaign, "chain_identity": identity, "dao_policy": policy,
                         "converter_profile": profile, "markets": markets, "feeds": list(feeds.values()),
                         "registrations": list(registrations.values()), "dependency_consumers": dependencies,
                         "oracle_snapshots": oracle_snapshots,
                         "discovery": {"registries": discovered, "reviewed_accounts": sorted(explicit),
                                       "claim": "explicit inspected scope only; not all consumers of public feeds"}})
    evidence = {"intent_digest": intent["intent_digest"], "markets": sorted(selected), "feeds": len(feeds),
                "production_executable": False, "admission": "controlled local fixture only; oracle pause is not maintenance mode"}
    run.confirm("00-prepare", "stage", evidence)
    return evidence


if __name__ == "__main__":
    stage_main("00-prepare", handle)
