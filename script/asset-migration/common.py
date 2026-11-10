"""Durable runtime for the fixed eight-stage, local-fixture migration procedure.

The native manager owns RPC, authorization, plan encoding and MPC verification.
This module only orders those commands and preserves their evidence. A journal is
not a revocation mechanism: an unresolved submission always blocks a replacement.
"""
from __future__ import annotations

import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
import fcntl
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.parse
import uuid


class Stop(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code
        self.message = message


def require(condition, message):
    if not condition:
        raise Stop(20, message)


def registration_funded(registration, balance):
    """Match storage.ensureDeposit's minimum-total no-op, including zero targets."""
    total = int(balance["total"]) if balance is not None else 0
    return total >= int(registration["minimum"])


def token_amount(value):
    require(isinstance(value, str) and value.isdecimal(), "noncanonical token amount")
    return int(value)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False,
                      allow_nan=False).encode("utf-8")


def canonical_dao_policy(policy):
    """Normalize native HashSets only; role order affects duplicate-name rules."""
    roles = []
    for role in policy["roles"]:
        kind = role["kind"]
        if isinstance(kind, dict) and "Group" in kind:
            kind = {**kind, "Group": sorted(kind["Group"])}
        roles.append({**role, "kind": kind, "permissions": sorted(role["permissions"])})
    return {**policy, "roles": roles}


def digest(value):
    return "sha256:" + hashlib.sha256(b"templar-asset-migration-v1\0" + canonical(value)).hexdigest()


def file_digest(path):
    return "sha256:" + hashlib.sha256(Path(path).read_bytes()).hexdigest()


_BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def hash_bytes(value):
    """Normalize native hex/base58 hashes; never decode a transaction payload."""
    require(isinstance(value, str), "hash is not text")
    if len(value) == 64 and all(char in "0123456789abcdefABCDEF" for char in value):
        return bytes.fromhex(value)
    number = 0
    for char in value:
        require(char in _BASE58, "invalid base58 hash")
        number = number * 58 + _BASE58.index(char)
    decoded = (b"\0" * (len(value) - len(value.lstrip("1")))
               + (number.to_bytes((number.bit_length() + 7) // 8, "big") if number else b""))
    require(len(decoded) == 32, "hash is not 32 bytes")
    return decoded


def encode_hash(value):
    raw = hash_bytes(value)
    number = int.from_bytes(raw, "big")
    encoded = ""
    while number:
        number, remainder = divmod(number, 58)
        encoded = _BASE58[remainder] + encoded
    return "1" * (len(raw) - len(raw.lstrip(b"\0"))) + encoded


def _object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"duplicate JSON field: {key}")
        result[key] = value
    return result


def parse_json(data):
    return json.loads(data, object_pairs_hook=_object,
                      parse_constant=lambda value: (_ for _ in ()).throw(Stop(20, f"invalid JSON constant: {value}")))


CAMPAIGN_REQUIRED = {
    "schema", "environment", "network", "rpc_url", "expected_chain_id", "expected_genesis_hash",
    "dao", "mpc_contract", "mpc_key_type", "submitter", "relayer", "converter_profile",
    "converter_account", "converter_code_sha256", "approval_wait_seconds", "finality_wait_seconds",
    "dependency_scope_accounts",
}
CAMPAIGN_OPTIONAL = {"markets", "conversions", "mpc_transport", "mpc_valid_for_blocks", "registry_accounts", "external_consumers"}


def campaign_header(campaign):
    _keys(campaign, CAMPAIGN_REQUIRED, CAMPAIGN_OPTIONAL, "campaign")
    require(type(campaign["schema"]) is int and campaign["schema"] == 1, "unsupported campaign schema")
    require(campaign["environment"] in ("sandbox", "production"), "unknown campaign environment")
    require(campaign["network"] in ("mainnet", "testnet"), "unknown manager network")
    require(campaign["mpc_key_type"] in ("ed25519", "secp256k1"), "unknown MPC key type")
    require(campaign["converter_profile"] == "local-one-to-one", "unimplemented converter profile")
    require(campaign.get("mpc_transport", "delegate-action") in ("delegate-action", "transaction"),
            "unknown MPC transport")
    for name in ("approval_wait_seconds", "finality_wait_seconds"):
        require(type(campaign[name]) is int and campaign[name] > 0, f"invalid {name}")


def text(value, label):
    require(isinstance(value, str) and value and value == value.strip(), f"invalid {label}")
    return value


def config_asset(value):
    # Representation conversion only. Standard-specific calls remain in the Rust gateway.
    reference = asset_reference(value)
    if reference["standard"] == "ft":
        return {"Nep141": reference["contract_id"]}
    return {"Nep245": {"contract_id": reference["contract_id"], "token_id": reference["token_id"]}}


def normalize_campaign(args):
    require(args.campaign is not None, "a shared --campaign TOML file is required for preparation")
    with Path(args.campaign).open("rb") as source:
        campaign = tomllib.load(source)
    campaign_header(campaign)
    for key in CAMPAIGN_REQUIRED - {"schema", "approval_wait_seconds", "finality_wait_seconds", "dependency_scope_accounts"}:
        text(campaign[key], key)
    url = urllib.parse.urlsplit(campaign["rpc_url"])
    require(url.scheme in ("http", "https") and url.hostname is not None and
            url.username is None and url.password is None and not url.query and not url.fragment,
            "RPC URL must be absolute and credential-free; use the existing RPC_API_KEY environment")
    checksum = campaign["converter_code_sha256"].lower()
    require(len(checksum) == 64 and all(char in "0123456789abcdef" for char in checksum), "invalid converter SHA-256")
    campaign["converter_code_sha256"] = checksum
    for key in ("dependency_scope_accounts", "registry_accounts", "external_consumers"):
        values = campaign.setdefault(key, [])
        require(isinstance(values, list) and all(isinstance(value, str) and value for value in values), f"invalid {key}")
        require(len(values) == len(set(values)), f"duplicate {key}")
        campaign[key] = sorted(values)
    transport = campaign.setdefault("mpc_transport", "delegate-action")
    if "mpc_valid_for_blocks" in campaign:
        require(transport == "delegate-action" and type(campaign["mpc_valid_for_blocks"]) is int and
                campaign["mpc_valid_for_blocks"] > 0, "explicit validity blocks require delegate-action")
    markets = campaign.setdefault("markets", [])
    require(isinstance(markets, list), "markets must be a list")
    if args.market is not None:
        require(not markets, "--market overrides conflict with campaign markets")
        market = {"account": args.market}
        for role in ("borrow", "collateral"):
            value = getattr(args, f"{role}_asset")
            if value is not None:
                market[f"{role}_asset"] = value
        markets.append(market)
    else:
        require(args.borrow_asset is None and args.collateral_asset is None, "asset overrides require --market")
    require(markets, "at least one market is required")
    seen = set()
    for market in markets:
        require(isinstance(market, dict) and not market.keys() - {"account", "mpc_path", "borrow_asset", "collateral_asset"},
                "unknown market fields")
        account = text(market.get("account"), "market account")
        require(account not in seen, "duplicate market")
        seen.add(account)
        require(any(f"{role}_asset" in market for role in ("borrow", "collateral")), "market needs at least one asset override")
        for role in ("borrow", "collateral"):
            if f"{role}_asset" in market:
                market[f"{role}_asset"] = config_asset(market[f"{role}_asset"])
        if "mpc_path" in market:
            text(market["mpc_path"], "MPC path")
    conversions = campaign.setdefault("conversions", [])
    require(isinstance(conversions, list) and conversions, "explicit conversion mappings are required")
    inputs = set()
    for conversion in conversions:
        require(isinstance(conversion, dict) and set(conversion) == {"old_asset", "new_asset"}, "invalid conversion mapping")
        conversion["old_asset"] = config_asset(conversion["old_asset"])
        conversion["new_asset"] = config_asset(conversion["new_asset"])
        require(conversion["old_asset"] != conversion["new_asset"], "unchanged conversion mapping")
        source = canonical(conversion["old_asset"])
        require(source not in inputs, "duplicate or ambiguous source conversion mapping")
        inputs.add(source)
    campaign["markets"] = sorted(markets, key=lambda value: value["account"])
    return campaign


def _keys(value, required, optional=(), label="object"):
    require(isinstance(value, dict), f"{label} must be an object")
    require(set(required) <= set(value) <= set(required) | set(optional), f"invalid {label} fields")


def asset_reference(asset):
    if isinstance(asset, str):
        parts = asset.split(":", 2)
        if len(parts) == 2 and parts[0] == "nep141" and parts[1]:
            return {"standard": "ft", "contract_id": parts[1]}
        if len(parts) == 3 and parts[0] == "nep245" and parts[1] and parts[2]:
            return {"standard": "mt", "contract_id": parts[1], "token_id": parts[2]}
    elif isinstance(asset, dict):
        if set(asset) == {"Nep141"} and isinstance(asset["Nep141"], str):
            return asset_reference("nep141:" + asset["Nep141"])
        if set(asset) == {"Nep245"}:
            value = asset["Nep245"]
            _keys(value, ("contract_id", "token_id"), label="NEP-245 asset")
            return asset_reference(f"nep245:{value['contract_id']}:{value['token_id']}")
        if asset.get("standard") == "ft":
            _keys(asset, ("standard", "contract_id"), label="FT reference")
            return asset_reference("nep141:" + asset["contract_id"])
        if asset.get("standard") == "mt":
            _keys(asset, ("standard", "contract_id", "token_id"), label="MT reference")
            return asset_reference(f"nep245:{asset['contract_id']}:{asset['token_id']}")
    raise Stop(20, "unsupported canonical asset identity")



def validate_campaign(campaign):
    campaign_header(campaign)
    validity = campaign.get("mpc_valid_for_blocks")
    require(validity is None or type(validity) is int and validity > 0
            and campaign.get("mpc_transport", "delegate-action") == "delegate-action",
            "mpc_valid_for_blocks requires positive delegate-action validity")
    for name in ("registry_accounts", "external_consumers"):
        values = campaign.get(name, [])
        require(isinstance(values, list) and all(isinstance(value, str) for value in values)
                and len(set(values)) == len(values), f"invalid {name}")
    url = urllib.parse.urlsplit(campaign["rpc_url"])
    require(url.scheme in ("http", "https") and url.hostname and not url.username and not url.password
            and not url.query and not url.fragment, "RPC URL must not contain credentials/query/fragment")
    require(isinstance(campaign["dependency_scope_accounts"], list)
            and len(set(campaign["dependency_scope_accounts"])) == len(campaign["dependency_scope_accounts"]),
            "duplicate dependency scope")
    for market in campaign.get("markets", []):
        _keys(market, ("account",), ("borrow_asset", "collateral_asset", "mpc_path"), "campaign market")
        require("borrow_asset" in market or "collateral_asset" in market, "market has no requested change")
        for role in ("borrow_asset", "collateral_asset"):
            if role in market:
                asset_reference(market[role])
    require(len({m["account"] for m in campaign.get("markets", [])}) == len(campaign.get("markets", [])),
            "duplicate campaign market")
    for route in campaign.get("conversions", []):
        _keys(route, ("old_asset", "new_asset"), label="campaign conversion")
        require(asset_reference(route["old_asset"]) != asset_reference(route["new_asset"]), "unchanged conversion")
    return campaign


def _fsync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def _publish(path, data):
    """Publish complete bytes without ever replacing an existing inode."""
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        require(path.is_file() and not path.is_symlink() and path.read_bytes() == data,
                f"immutable artifact differs: {path}")
        return
    fd, temporary = tempfile.mkstemp(prefix=".publish-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        try:
            os.link(temporary, path)
        except FileExistsError:
            require(path.read_bytes() == data, f"immutable publication conflict: {path}")
        _fsync_directory(path.parent)
    finally:
        os.unlink(temporary)


def _append(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("ab") as stream:
        stream.write(canonical(value) + b"\n")
        stream.flush()
        os.fsync(stream.fileno())
    _fsync_directory(path.parent)


def _anchor_path(path):
    return path.with_name("." + path.name + ".tip.json")


def _check_anchor(path, sequence, checksum, binding):
    anchor = _anchor_path(path)
    require(not anchor.is_symlink(), "journal/lease anchor is a symlink")
    if sequence == 0:
        require(not anchor.exists(), "journal/lease was removed or truncated to zero")
        return
    if not anchor.exists():
        raise Stop(30, "journal/lease anchor missing; inspect interrupted durable append")
    expected = {"schema": 1, "sequence": sequence, "hash": checksum, "binding": binding}
    if parse_json(anchor.read_bytes()) != expected:
        raise Stop(30, "journal/lease tail differs from durable anchor; never reset or resubmit")


def _append_anchored(path, value, checksum, binding):
    _append(path, value)
    anchor = _anchor_path(path)
    expected = {"schema": 1, "sequence": value["sequence"], "hash": checksum, "binding": binding}
    fd, temporary = tempfile.mkstemp(prefix=".anchor-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(canonical(expected) + b"\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, anchor)
        _fsync_directory(path.parent)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def _lock(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    stream = path.open("a+b")
    try:
        fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        stream.close()
        raise Stop(20, f"another local runner holds lock: {path}")
    return stream


def verify_receipts(result):
    require(isinstance(result, dict) and result.get("status") == "succeeded", "transaction is not finally successful")
    require(result.get("failed_receipts") == [], "transaction contains failed child receipts")
    receipts = result.get("receipts")
    require(isinstance(receipts, list) and bool(receipts), "transaction has no terminal receipt evidence")
    require(all(receipt.get("status") == "Succeeded" for receipt in receipts), "non-success child receipt")
    return result


@dataclass(frozen=True)
class JournalOperation:
    key: str
    operation: str


def journal_identity(key):
    if isinstance(key, JournalOperation):
        require(key.operation in ("dao", "mpc"), "unknown journal operation scope")
        return key.key, key.operation
    require(isinstance(key, str), "journal domain key must be text")
    return key, None


def frozen_market_metadata(market):
    for role in market["roles"]:
        for side in ("old", "new"):
            yield role[f"{side}_reference"], role[f"{side}_metadata"]
    for asset in market["unchanged_assets"]:
        yield asset["reference"], asset["metadata"]


class Run:
    def __init__(self, args, stage=None):
        self.args = args
        self.stage = stage
        self.root = Path(args.run_dir).resolve()
        self.root.mkdir(parents=True, exist_ok=True)
        self.submit = bool(args.submit)
        self.wait = bool(args.wait)
        self.intent = {}
        self.campaign = {}
        self.artifacts = {}
        self.events = []
        self._locks = []
        self._key_locks = set()
        self._run_lock = _lock(self.root / ".run.lock")
        self._state_root = Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state"))) / "templar-asset-migration"
        self._state_root.mkdir(parents=True, exist_ok=True, mode=0o700)
        self._refs = None
        intent_path = self.root / "intent.json"
        if intent_path.exists():
            intent = self._parse_intent(parse_json(intent_path.read_bytes()))
            self._verify_artifacts(intent["artifacts"])
            self.configure(intent["campaign"])
            self.intent = intent
            self.artifacts = dict(intent["artifacts"])
            self._read_journal()
            self._verify_artifacts(self.artifacts)
            self._lock_keys()
        else:
            require(stage == "00-prepare", "missing frozen intent; run preparation first")
            require(not (self.root / "journal.jsonl").exists(), "journal exists without frozen intent")

    def close(self):
        for stream in reversed(self._locks):
            stream.close()
        self._run_lock.close()

    def path(self, relative):
        require(isinstance(relative, (str, Path)), "artifact path is not text")
        candidate = Path(relative)
        require(not candidate.is_absolute() and ".." not in candidate.parts, "artifact path escapes run directory")
        path = self.root / candidate
        require(path.resolve().is_relative_to(self.root), "artifact symlink escapes run directory")
        require(not any(part.is_symlink() for part in (path, *path.parents) if part != self.root.parent),
                "symlink in immutable artifact path")
        return path

    def relative(self, path):
        path = Path(path).absolute()
        require(path.is_relative_to(self.root), "path is outside run directory")
        name = str(path.relative_to(self.root))
        self.path(name)
        return name

    def configure(self, campaign):
        validate_campaign(campaign)
        if self.campaign:
            require(self.campaign == campaign, "campaign differs from frozen intent")
        self.campaign = campaign
        return campaign

    def freeze(self, intent):
        require(self.campaign and intent["campaign"] == self.campaign, "unconfigured/different campaign")
        value = dict(intent)
        value["schema"] = 1
        value["artifacts"] = {**self.artifacts, **value.get("artifacts", {})}
        value.pop("intent_digest", None)
        value["intent_digest"] = digest(value)
        self._parse_intent(value)
        self._verify_artifacts(value["artifacts"])
        if self.intent:
            require(value == self.intent, "cannot replace frozen intent")
            return self.intent
        _publish(self.root / "intent.json", canonical(value) + b"\n")
        self.intent = value
        self.artifacts = dict(value["artifacts"])
        self._lock_keys()
        self.record("00-prepare", "intent", "prepared", {"intent_digest": value["intent_digest"]})
        return value

    def _parse_intent(self, value):
        _keys(value, ("schema", "campaign", "markets", "feeds", "artifacts", "intent_digest",
                      "dao_policy", "oracle_snapshots"),
              ("chain_identity", "converter_profile", "registrations",
               "dependency_consumers", "discovery"), "intent")
        require(type(value["schema"]) is int and value["schema"] == 1, "unsupported intent schema")
        require(value["intent_digest"] == digest({k: v for k, v in value.items() if k != "intent_digest"}),
                "intent digest mismatch")
        validate_campaign(value["campaign"])
        accounts = set()
        require(isinstance(value["markets"], list) and bool(value["markets"]), "intent has no markets")
        for market in value["markets"]:
            required = {"account", "mpc_path", "public_key", "initial_nonce", "access_keys", "configuration",
                        "expected_configuration", "account_identity", "snapshot", "changes", "roles", "unchanged_assets"}
            require(isinstance(market, dict) and required <= market.keys(), "incomplete frozen market record")
            require(market["account"] not in accounts, "duplicate intent market")
            accounts.add(market["account"])
            require(type(market["initial_nonce"]) is int and market["initial_nonce"] >= 0,
                    "invalid initial derived-key nonce")
            keys = market["access_keys"]
            require(isinstance(keys, list) and keys
                    and len({entry["public_key"] for entry in keys}) == len(keys), "invalid frozen access-key set")
            selected = [entry for entry in keys if entry["public_key"] == market["public_key"]]
            require(len(selected) == 1 and selected[0]["permission"] == "FullAccess"
                    and selected[0]["nonce"] == market["initial_nonce"], "inconsistent frozen MPC access key")
            require(isinstance(market["roles"], list) and bool(market["roles"]), "market has no changed roles")
            changed = set()
            expected = dict(market["configuration"])
            for role in market["roles"]:
                require(role["role"] in ("borrow", "collateral") and role["role"] not in changed,
                        "invalid/duplicate changed market role")
                changed.add(role["role"])
                field = role["role"] + "_asset"
                require(role["old_asset"] == market["configuration"][field]
                        and role["new_asset"] == market["expected_configuration"][field]
                        and role["old_asset"] != role["new_asset"], "role/configuration identities differ")
                require(asset_reference(role["old_asset"]) == role["old_reference"]
                        and asset_reference(role["new_asset"]) == role["new_reference"],
                        "token reference/configuration identity mismatch")
                require(type(role["route_id"]) is int and role["route_id"] >= 0
                        and role.get("inventory_role", role["role"]) in ("borrow", "collateral"),
                        "invalid route/inventory identity")
                for side in ("old", "new"):
                    amount = role[f"initial_{side}_balance"]
                    require(isinstance(amount, str) and amount.isdecimal(), "invalid frozen raw-unit balance")
                expected[field] = role["new_asset"]
            require(expected == market["expected_configuration"], "intent changes unrelated configuration fields")
            unchanged = market["unchanged_assets"]
            require(isinstance(unchanged, list), "missing unchanged asset observations")
            frozen_roles = set()
            for asset in unchanged:
                role = asset["role"]
                require(role in ("borrow", "collateral") and role not in changed and role not in frozen_roles,
                        "invalid/duplicate unchanged asset role")
                frozen_roles.add(role)
                require(asset["reference"] == asset_reference(market["configuration"][role + "_asset"])
                        and isinstance(asset["balance"], str) and asset["balance"].isdecimal(),
                        "invalid unchanged asset identity or balance")
            require(changed | frozen_roles == {"borrow", "collateral"}, "not every asset inventory is frozen")
        require(accounts == {market["account"] for market in value["campaign"]["markets"]},
                "campaign/intent market cohort mismatch")
        seen = set()
        require(isinstance(value.get("oracle_snapshots"), dict)
                and set(value["oracle_snapshots"]) == {feed["oracle"] for feed in value["feeds"]},
                "missing initial oracle effective-code snapshots; prepare a new run")
        for feed in value["feeds"]:
            identity = (feed["oracle"], feed["feed"])
            require(identity not in seen and isinstance(feed["feed"], str) and len(feed["feed"]) == 64
                    and all(char in "0123456789abcdef" for char in feed["feed"]), "invalid/duplicate feed identity")
            require(type(feed["initial_manual"]) is bool
                    and feed["initial_breaker"]["is_manually_tripped"] == feed["initial_manual"],
                    "initial manual pause context differs")
            seen.add(identity)
        dependencies = [consumer["account"] for consumer in value.get("dependency_consumers", [])]
        require(len(set(dependencies)) == len(dependencies) and not accounts.intersection(dependencies),
                "duplicate/modified dependency consumer")
        return value

    def _read_journal(self):
        path = self.root / "journal.jsonl"
        if not path.exists():
            _check_anchor(path, 0, None, self.intent["intent_digest"])
            return
        data = path.read_bytes()
        require(not data or data.endswith(b"\n"), "truncated journal; reconcile before proceeding")
        previous = None
        for number, line in enumerate(data.splitlines(), 1):
            require(bool(line), "blank/truncated journal record")
            event = parse_json(line)
            _keys(event, ("schema", "sequence", "intent_digest", "stage", "key", "operation", "attempt", "event",
                          "evidence", "artifacts", "previous_hash", "event_hash"), label="journal event")
            require(event["schema"] == 1 and event["sequence"] == number
                    and event["intent_digest"] == self.intent["intent_digest"] and event["previous_hash"] == previous,
                    "journal sequence/intent/hash-chain mismatch")
            require(event["event_hash"] == digest({k: v for k, v in event.items() if k != "event_hash"}), "edited journal record")
            require(event["event"] in ("prepared", "attempt_started", "submitted", "waiting", "confirmed", "failed", "unknown", "superseded"),
                    "unknown journal event")
            require(event["operation"] in (None, "dao", "mpc"), "unknown journal operation scope")
            require(type(event["attempt"]) is int and event["attempt"] > 0
                    and event["stage"] in (*STAGES, "runtime") and isinstance(event["evidence"], dict),
                    "invalid journal stage/attempt/evidence")
            if event["operation"] and event["event"] in ("attempt_started", "submitted", "unknown"):
                phase = event["evidence"].get("phase")
                require(phase in ("proposal", "execute")
                        and (event["operation"] == "mpc" or phase == "proposal"), "invalid financial phase")
                if event["event"] != "attempt_started":
                    require(event["evidence"].get("reconciled") is True or any(
                        old["stage"] == event["stage"] and old["key"] == event["key"]
                        and old["operation"] == event["operation"] and old["attempt"] == event["attempt"]
                        and old["event"] == "attempt_started" and old["evidence"]["phase"] == phase
                        for old in self.events), "financial result has no durable submission boundary")
            if event["operation"] and event["event"] == "attempt_started":
                require(not any(old["stage"] == event["stage"] and old["key"] == event["key"]
                                and old["operation"] == event["operation"] and old["attempt"] == event["attempt"]
                                and old["event"] == "attempt_started"
                                and old["evidence"]["phase"] == event["evidence"]["phase"]
                                for old in self.events), "duplicate financial submission attempt")
            for name, checksum in event["artifacts"].items():
                require(name not in self.artifacts or self.artifacts[name] == checksum, "artifact digest changed in journal")
                self.artifacts[name] = checksum
            self.events.append(event)
            previous = event["event_hash"]
        _check_anchor(path, len(self.events), previous, self.intent["intent_digest"])

    def _verify_artifacts(self, artifacts):
        for name, checksum in artifacts.items():
            path = self.path(name)
            require(path.is_file() and file_digest(path) == checksum, f"missing/edited immutable artifact: {name}")


    def _collect(self, path, seen=None):
        name = self.relative(path)
        path = self.path(name)
        require(path.is_file(), f"missing artifact: {name}")
        if seen is None:
            seen = set()
        require(name not in seen, f"cyclic artifact references: {name}")
        seen.add(name)
        found = {name: file_digest(path)}
        if path.suffix in (".json", ".toml"):
            value = parse_json(path.read_bytes()) if path.suffix == ".json" else tomllib.loads(path.read_text())
            # Native specifications have finite extends/blob trees. Reject cycles
            # separately before descending so hand-edited links cannot recurse.
            found.update(self._references_guarded(value, path.parent, seen))
        seen.remove(name)
        return found

    def _references_guarded(self, value, base, seen):
        found = {}
        if isinstance(value, dict):
            for key, item in value.items():
                if key == "spec" and isinstance(item, dict) and isinstance(value.get("source_path"), str):
                    source = Path(value["source_path"])
                    source = source if source.is_absolute() else base / source
                    require(source.resolve().is_relative_to(self.root), "embedded spec source escapes run")
                    found.update(self._references_guarded(item, source.parent, seen))
                    continue
                names = item if key == "extends" and isinstance(item, list) else [item] if key in (
                    "file", "source_path", "payload_file", "plan_path", "snapshot", "baseline", "changes") and isinstance(item, str) else []
                for name in names:
                    path = Path(name)
                    if not path.is_absolute():
                        path = (base if key in ("file", "extends", "source_path", "payload_file") else self.root) / path
                    require(path.resolve().is_relative_to(self.root), f"referenced artifact escapes run: {name}")
                    found.update(self._collect(path, seen))
                if not names:
                    found.update(self._references_guarded(item, base, seen))
        elif isinstance(value, list):
            for item in value:
                found.update(self._references_guarded(item, base, seen))
        return found

    def seal_tree(self, path):
        path = self.path(self.relative(path))
        paths = [path] if path.is_file() else sorted(p for p in path.rglob("*") if p.is_file())
        found = {}
        for child in paths:
            found.update(self._collect(child))
        self._register(found)
        return found

    def _register(self, found):
        for name, checksum in found.items():
            require(name not in self.artifacts or self.artifacts[name] == checksum, f"immutable artifact edited: {name}")
        new = {name: checksum for name, checksum in found.items() if name not in self.artifacts}
        if new and self.intent:
            self._event("runtime", "artifacts", "prepared", {}, new)
        self.artifacts.update(found)

    def save(self, relative, value):
        path = self.path(relative)
        _publish(path, canonical(value) + b"\n")
        self._register(self._collect(path))
        return path

    def load(self, relative):
        path = self.path(relative)
        require(str(relative) in self.artifacts, f"unsealed artifact: {relative}")
        found = self._collect(path)
        require(all(self.artifacts.get(name) == checksum for name, checksum in found.items()), "artifact/reference edited")
        return parse_json(path.read_bytes()) if path.suffix != ".toml" else tomllib.loads(path.read_text())

    def operation_dir(self, stage, key):
        require(stage in STAGES or stage == "runtime", "unknown fixed stage")
        if isinstance(key, JournalOperation):
            domain_key, operation = journal_identity(key)
            key = domain_key + ":" + operation
        directory = self.path(f"{stage}/{hashlib.sha256(key.encode()).hexdigest()}")
        directory.mkdir(parents=True, exist_ok=True)
        return directory

    def _event(self, stage, key, event, evidence, artifacts=None, attempt=1):
        require(bool(self.intent), "cannot journal before freezing intent")
        require(event in ("prepared", "attempt_started", "submitted", "waiting", "confirmed", "failed", "unknown", "superseded"), "unknown event kind")
        key, operation = journal_identity(key)
        value = {"schema": 1, "sequence": len(self.events) + 1, "intent_digest": self.intent["intent_digest"],
                 "stage": stage, "key": key, "operation": operation, "attempt": attempt, "event": event, "evidence": evidence,
                 "artifacts": artifacts or {}, "previous_hash": self.events[-1]["event_hash"] if self.events else None}
        value["event_hash"] = digest(value)
        _append_anchored(self.root / "journal.jsonl", value, value["event_hash"], self.intent["intent_digest"])
        self.events.append(value)
        return value

    def record(self, stage, key, event, evidence):
        self._verify_artifacts(self.artifacts)
        return self._event(stage, key, event, evidence)

    def confirm(self, stage, key, evidence):
        previous = self.confirmed(stage, key)
        if previous == evidence:
            return evidence
        self.record(stage, key, "confirmed", evidence)
        return evidence

    def confirmed(self, stage, key):
        # Ingress, new evidence, and write admission verify artifacts; this is
        # only a lookup in the already-verified in-memory journal.
        key, operation = journal_identity(key)
        for event in reversed(self.events):
            if event["stage"] == stage and event["key"] == key and event["operation"] == operation:
                return event["evidence"] if event["event"] == "confirmed" else None
        return None

    def require_stage(self, stage):
        require(self.confirmed(stage, "stage") is not None, f"required stage incomplete: {stage}")

    def _invoke(self, argv, signer=None):
        require(self.campaign, "campaign is not configured")
        require(isinstance(argv, list) and all(isinstance(arg, str) for arg in argv), "command must be an argument array")
        require("--secret-key" not in argv and not any(arg.startswith("--secret-key=") for arg in argv), "secret argv prohibited")
        command = [os.environ.get("ASSET_MIGRATION_TMPLRMGR", os.environ.get("TMPLRMGR", "tmplrmgr")),
                   "--network", self.campaign["network"], "--rpc-url", self.campaign["rpc_url"], *argv]
        environment = os.environ.copy()
        if signer is not None:
            role = "RELAYER" if argv[:3] == ["dao", "mpc", "relay"] else "SUBMITTER"
            secret = environment.get(f"ASSET_MIGRATION_{role}_SECRET_KEY")
            if secret:
                environment["SECRET_KEY"] = secret
        try:
            financial = "--print" not in argv and (argv[:2] == ["dao", "propose"]
                        or argv[:3] in (["dao", "mpc", "propose"], ["dao", "mpc", "relay"], ["dao", "mpc", "broadcast"]))
            timeout = self.campaign["finality_wait_seconds"] if financial or argv[:2] == ["read", "tx.get"] else None
            result = subprocess.run(command, capture_output=True, env=environment, check=False, timeout=timeout)
        except subprocess.TimeoutExpired as error:
            result = subprocess.CompletedProcess(command, 124, stdout=error.stdout or b"", stderr=error.stderr or b"")
        except OSError as error:
            raise Stop(20, f"cannot start manager: {error}") from error
        # Credentials are never recorded, including the child environment. Native
        # stderr is diagnostic only; stdout is the sole structured evidence.
        if result.stderr:
            sys.stderr.buffer.write(result.stderr)
            sys.stderr.buffer.flush()
        try:
            output = parse_json(result.stdout) if result.stdout.strip() else None
        except (ValueError, UnicodeError, Stop):
            output = None
        return {"argv": argv, "returncode": result.returncode, "stdout": result.stdout.decode("utf-8", errors="replace"),
                "output": output}

    def command(self, argv, signer=None, json_output=True):
        readonly = (argv[:1] in (["read"], ["read-batch"])
                    or argv[:2] in (["patch", "export"], ["patch", "plan"], ["patch", "dry-run"],
                                   ["proxy-oracle", "predict-manual-trip"])
                    or argv[:3] in (["market", "generate-patch", "config"],
                                   ["dao", "mpc", "derive-key"], ["dao", "mpc", "show"]))
        printed = "--print" in argv and argv[argv.index("--print") + 1:argv.index("--print") + 2] == ["json"]
        require(readonly or printed, "submitting commands require the journaled financial boundary")
        result = self._invoke(argv, signer)
        require(result["returncode"] == 0, f"manager command failed: {' '.join(argv[:3])}")
        if json_output:
            require(result["output"] is not None or result["stdout"].strip() == "null", "manager did not return JSON")
            return result["output"]
        return result["stdout"]

    def read(self, method, params):
        return self.command(["read", method, "--json", canonical(params).decode()])

    def read_many(self, requests):
        if not requests:
            return []
        values = self.command(["read-batch", "--json", canonical([
            {"method": method, "params": params} for method, params in requests
        ]).decode()])
        require(isinstance(values, list) and len(values) == len(requests), "native read batch returned mismatched results")
        return values

    def plan_cmd(self, argv, signer, path, *, subcommand=()):
        name = self.relative(path)
        if path.exists():
            self.load(name)
            return path
        value = self.command([*argv, "--signer-id", signer, "--print", "json", *subcommand])
        return self.save(name, value)

    def plan(self, method, params, signer, path):
        return self.plan_cmd(["write", method, "--json", canonical(params).decode()], signer, path)

    def identity(self, account):
        result = self.read("account.get", {"account_id": account})
        return {name: result.get(name) for name in ("code_hash", "global_contract_hash", "global_contract_account_id")}

    def balance(self, reference, account):
        value = self.read("token.getBalanceOf", {"token": asset_reference(reference), "account_id": account})
        return token_amount(value)

    @staticmethod
    def view_params(account, method, args=None):
        return {"contract_id": account, "method_name": method,
                "args": {"encoding": "json", "value": args or {}}}

    def view(self, account, method, args=None):
        return self.read("contract.viewFunction", self.view_params(account, method, args))

    def parse_snapshot(self, relative, expected_account=None, expected_count=None):
        path = self.path(relative)
        require(relative in self.artifacts, "snapshot is not sealed")
        spec = self.load(relative)
        _keys(spec, ("schema", "account_id"), ("check", "op", "extends"), "snapshot export")
        require(spec["schema"] == 3 and not spec.get("op") and not spec.get("extends"), "not a complete raw export")
        if expected_account is not None:
            require(spec["account_id"] == expected_account, "snapshot belongs to different account")
        entries = {}
        for check in spec.get("check", []):
            _keys(check, ("key", "expect"), label="snapshot storage check")
            _keys(check["key"], ("base64",), label="raw snapshot key")
            key = base64.b64decode(check["key"]["base64"], validate=True)
            require(key not in entries, "duplicate raw storage key")
            expected = check["expect"]
            _keys(expected, ("file",), label="raw snapshot value")
            blob = path.parent / expected["file"]
            name = self.relative(blob)
            require(name in self.artifacts and file_digest(self.path(name)) == self.artifacts[name], "missing/edited raw snapshot blob")
            entries[key] = blob.read_bytes()
        if expected_count is not None:
            require(len(entries) == expected_count, "snapshot entry count mismatch")
        return entries

    def snapshot(self, account, relative_prefix):
        relative = relative_prefix if relative_prefix.endswith(".toml") else relative_prefix + ".toml"
        path = self.path(relative)
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.exists():
            report = self.load(relative + ".report.json")
        else:
            report = self.command(["patch", "export", account, "--out", str(path)])
            require(Path(report["spec"]).resolve() == path.resolve(), "export published a different path")
            self.seal_tree(path)
            self.seal_tree(Path(report["blobs"]))
            self.save(relative + ".report.json", report)
        effective_code_sha256 = self.snapshot_code_hash(relative)
        if self.intent:
            initial = next((item["snapshot"] for item in
                            [*self.intent["markets"], *self.intent.get("dependency_consumers", [])]
                            if item["account"] == account), None)
            if initial is None:
                initial = self.intent["oracle_snapshots"].get(account)
            if initial is not None:
                require(effective_code_sha256 == self.snapshot_code_hash(initial),
                        f"effective contract code drift: {account}")
        return self.parse_snapshot(relative, account, report["entries"]), relative

    def snapshot_code_hash(self, relative):
        report = self.load(relative + ".report.json")
        value = report.get("effective_code_sha256")
        require(isinstance(value, str) and len(value) == 64
                and all(char in "0123456789abcdef" for char in value),
                "snapshot is missing native effective-code proof; prepare a new run")
        return value

    def snapshot_with_reads(self, account, relative_prefix, requests):
        """Run independent native read commands together, not Python RPC.

        Only the snapshot worker publishes artifacts/journal entries; the main
        thread's typed read batch is read-only. Join before inspecting run state.
        These observations are not one atomic chain snapshot.
        """
        with ThreadPoolExecutor(max_workers=1) as worker:
            snapshot = worker.submit(self.snapshot, account, relative_prefix)
            values = self.read_many(requests)
            storage, relative = snapshot.result()
        return storage, relative, values

    def market_storage(self, market):
        account = market["account"] if isinstance(market, dict) else market
        return self.snapshot(account, f"observations/{uuid.uuid4().hex}/{account}")[0]

    @staticmethod
    def storage_digest(entries):
        return digest([[base64.b64encode(key).decode(), base64.b64encode(value).decode()]
                       for key, value in sorted(entries.items())])

    def oracle_context(self, oracle):
        storage, snapshot = self.snapshot(oracle, f"observations/{uuid.uuid4().hex}/oracle")
        return {"identity": self.identity(oracle), "storage_digest": self.storage_digest(storage),
                "storage_entries": len(storage), "effective_code_sha256": self.snapshot_code_hash(snapshot)}

    def require_write_admission(self):
        require(self.submit, "submission is not authorized without --submit")
        self._verify_artifacts(self.artifacts)
        campaign = self.campaign
        require(campaign["environment"] == "sandbox" and campaign["converter_profile"] == "local-one-to-one",
                "production writes disabled: real converter and intermediate-state admission are not implemented")
        url = urllib.parse.urlsplit(campaign["rpc_url"])
        try:
            local = ipaddress.ip_address(url.hostname).is_loopback
        except ValueError:
            local = url.hostname == "localhost"
        require(local and not url.username and not url.password and not url.query and not url.fragment,
                "local converter requires an explicit credential-free loopback RPC URL")
        identity, converter, profile, native_policy = self.read_many([
            ("chain.getIdentity", None),
            ("account.get", {"account_id": campaign["converter_account"]}),
            ("contract.viewFunction", self.view_params(campaign["converter_account"], "get_profile")),
            ("dao.getPolicy", {"dao_id": campaign["dao"]}),
        ])
        require(identity["chain_id"] == campaign["expected_chain_id"]
                and identity["genesis_hash"] == campaign["expected_genesis_hash"], "chain/genesis identity changed")
        require(identity["chain_id"].lower() not in ("mainnet", "testnet", "betanet"), "local fixture cannot write to a production network")
        actual_hash = hash_bytes(converter["code_hash"]).hex()
        require(actual_hash == campaign["converter_code_sha256"].removeprefix("sha256:"), "converter code identity changed")
        _keys(profile, ("schema", "profile", "settlement_rule", "requires_registration", "routes"), label="converter profile")
        require(profile["schema"] == 1 and profile["profile"] == "local-one-to-one"
                and profile["settlement_rule"] == "one-to-one-raw" and profile["requires_registration"] is False,
                "unrecognized local converter semantics")
        require(profile == self.intent["converter_profile"], "frozen converter profile changed")
        self._registration_observations(required=self.stage not in ("00-prepare", "01-register"))
        metadata_checks = []
        for market in self.intent["markets"]:
            self._installed_key(market)
            active = self._lease(market, {}, inspect=True)
            require(active is None or active["run_dir"] == str(self.root)
                    and active["intent_digest"] == self.intent["intent_digest"],
                    "another campaign has an outstanding signature for this market key")
            for role in market["roles"]:
                matches = [route for route in profile["routes"] if route["id"] == role["route_id"]]
                require(len(matches) == 1 and matches[0]["old_asset"] == role["old_asset"]
                        and matches[0]["new_asset"] == role["new_asset"], "converter route changed")
            metadata_checks.extend(frozen_market_metadata(market))
        for consumer in self.intent.get("dependency_consumers", []):
            metadata_checks.extend((asset["reference"], asset["metadata"]) for asset in consumer["assets"])
        live_metadata = self.read_many([
            ("token.getMetadata", {"token": reference}) for reference, _expected in metadata_checks
        ])
        require(all(actual == expected for actual, (_reference, expected) in zip(live_metadata, metadata_checks)),
                "changed or unchanged asset metadata drift")
        policy = canonical_dao_policy(native_policy)
        require(policy == canonical_dao_policy(self.intent["dao_policy"]),
                "DAO policy changed since preparation")
        return {"chain": identity, "converter_profile": profile, "dao_policy": policy}

    def _installed_key(self, market):
        derived = self.command(["dao", "mpc", "derive-key", "--dao", self.campaign["dao"],
                                "--mpc-contract", self.campaign["mpc_contract"], "--key-type", self.campaign["mpc_key_type"],
                                "--account-id", market["account"], "--path", market["mpc_path"]])
        require(derived["public_key"] == market["public_key"], "derived public key changed")
        expected_nonce = int(market["initial_nonce"])
        for event in self.events:
            if event["operation"] == "mpc" and event["event"] == "confirmed":
                result = event["evidence"]
                if result["review"]["payload"]["signer_id"] == market["account"]:
                    expected_nonce = max(expected_nonce, int(result["nonce"]))
        expected = {entry["public_key"]: entry for entry in market["access_keys"]}
        expected[market["public_key"]] = {**expected[market["public_key"]], "nonce": expected_nonce}
        actual = self.read("account.listAccessKeys", {"account_id": market["account"]})
        require(len(actual) == len(expected) and {entry["public_key"]: entry for entry in actual} == expected,
                "market key set, permissions, or nonce changed outside confirmed campaign executions")
        return expected[market["public_key"]]

    def _lock_keys(self):
        for market in sorted(self.intent.get("markets", []), key=lambda value: value["account"]):
            key = self._key_name(market)
            if key not in self._key_locks:
                self._locks.append(_lock(self._state_root / (key + ".lock")))
                self._key_locks.add(key)

    def _key_name(self, market):
        return digest([self.campaign["expected_chain_id"], self.campaign["expected_genesis_hash"],
                       market["account"], market["public_key"]]).split(":")[1]

    def _lease(self, market, operation, release=False, proof=None, inspect=False):
        name = self._key_name(market)
        require(name in self._key_locks, "derived-key lock is not held")
        path = self._state_root / (name + ".leases.jsonl")
        active = None
        previous = None
        sequence = 0
        if path.exists():
            data = path.read_bytes()
            require(not data or data.endswith(b"\n"), "truncated outstanding-key lease")
            for sequence, line in enumerate(data.splitlines(), 1):
                value = parse_json(line)
                require(value["schema"] == 1 and value["sequence"] == sequence and value["previous_hash"] == previous
                        and value["hash"] == digest({k: v for k, v in value.items() if k != "hash"}), "edited key lease ledger")
                if value["event"] == "acquired":
                    require(active is None, "overlapping outstanding key leases")
                    active = value["owner"]
                else:
                    require(value["event"] == "released" and active == value["owner"] and value["proof"], "invalid lease release")
                    active = None
                previous = value["hash"]
        _check_anchor(path, sequence, previous, name)
        if inspect:
            return active
        owner = {"run_dir": str(self.root), "intent_digest": self.intent["intent_digest"], **operation}
        require(active is None or active == owner, "another campaign/operation has an unresolved payload for this key")
        if release and active is None or not release and active == owner:
            return
        value = {"schema": 1, "sequence": sequence + 1,
                 "previous_hash": previous, "event": "released" if release else "acquired", "owner": owner,
                 "proof": proof}
        require(not release or proof, "cannot release outstanding payload without on-chain proof")
        value["hash"] = digest(value)
        _append_anchored(path, value, value["hash"], name)

    def _operation_events(self, stage, key):
        key, operation = journal_identity(key)
        return [event for event in self.events if event["stage"] == stage
                and event["key"] == key and event["operation"] == operation]

    def _reference(self, stage, key, attempt, proposal_id=None):
        if self._refs is None:
            self._refs = []
            if self.args.execution_refs:
                value = parse_json(Path(self.args.execution_refs).read_bytes())
                _keys(value, ("schema", "intent_digest", "operations"), label="execution references")
                require(value["schema"] == 1 and value["intent_digest"] == self.intent["intent_digest"], "execution references belong to another intent")
                identities = set()
                for reference in value["operations"]:
                    _keys(reference, ("stage", "key", "attempt", "dao_proposal_id", "tx_hash", "sender_account_id"),
                          ("execution_tx_hash", "execution_sender_account_id"), "execution reference")
                    require(("execution_tx_hash" in reference) == ("execution_sender_account_id" in reference),
                            "execution identity requires both hash and sender")
                    identity = (reference["stage"], reference["key"], reference["attempt"])
                    require(identity not in identities and type(reference["attempt"]) is int and reference["attempt"] > 0
                            and type(reference["dao_proposal_id"]) is int and reference["dao_proposal_id"] >= 0,
                            "duplicate/invalid execution reference")
                    identities.add(identity)
                    self._refs.append(reference)
        matches = [reference for reference in self._refs if (reference["stage"], reference["key"], reference["attempt"]) == (stage, key, attempt)]
        if not matches:
            return None
        reference = matches[0]
        require(proposal_id is None or reference["dao_proposal_id"] == proposal_id, "execution reference names a different proposal")
        return reference

    def _tx(self, reference):
        deadline = time.monotonic() + (self.campaign["finality_wait_seconds"] if self.wait else 0)
        while True:
            try:
                result = self.read("tx.get", {"tx_hash": reference["tx_hash"],
                                             "sender_account_id": reference["sender_account_id"], "wait_until": "FINAL"})
            except Stop as error:
                raise Stop(30, "known transaction cannot yet be reconciled; never resubmit") from error
            self.save(f"observations/{uuid.uuid4().hex}/transaction.json", {"reference": reference, "result": result})
            if result["status"] != "pending":
                verify_receipts(result)
                transaction = result.get("transaction")
                require(isinstance(transaction, dict) and transaction["hash"] == reference["tx_hash"]
                        and transaction["signer_id"] == reference["sender_account_id"],
                        "finalized transaction identity differs from requested hash/sender")
                return result
            if time.monotonic() >= deadline:
                raise Stop(30, "known transaction is pending finality; never resubmit")
            time.sleep(min(1, max(0, deadline - time.monotonic())))

    @staticmethod
    def _transaction_reference(output):
        if not isinstance(output, dict):
            return None
        if output.get("tx_hash") and output.get("sender_account_id"):
            return {"tx_hash": output["tx_hash"], "sender_account_id": output["sender_account_id"]}
        operation = output.get("add_proposal", output).get("operation")
        if operation:
            for step in reversed(operation["steps"]):
                status = step["status"]
                if isinstance(status, dict):
                    value = next(iter(status.values()))
                    if value.get("tx_hash"):
                        return {"tx_hash": value["tx_hash"], "sender_account_id": operation["signer_account_id"]}
        return None

    @staticmethod
    def _returned_id(receipts, dao):
        values = []
        for receipt in receipts["receipts"]:
            value = receipt.get("return_value")
            if receipt["executor_id"] == dao and value and value["encoding"] == "json" and type(value["value"]) is int:
                values.append(value["value"])
        require(len(set(values)) == 1, "proposal submission has no unique returned ID")
        return values[0]

    def _submit_once(self, stage, key, attempt, phase, argv, signer, before_write=None, envelope=None, known_reference=None, acquire_lease=None):
        require(self.submit, "submission not authorized")
        self.require_write_admission()
        self._assert_no_ambiguous_submission()
        if before_write:
            before_write()
        if acquire_lease is not None:
            self._lease(*acquire_lease)
        self._event(stage, key, "attempt_started", {"phase": phase, "argv": argv,
                                                  "known_transaction": known_reference}, attempt=attempt)
        try:
            result = self._invoke(argv, signer)
        except BaseException:
            # Even local process errors after this durable boundary are unknown:
            # only explicit transaction evidence can prove what happened.
            raise Stop(30, "submission interrupted after durable attempt_started; reconcile, never resubmit")
        if envelope is not None and envelope.exists():
            self.seal_tree(envelope)
        evidence_path = self.operation_dir(stage, key) / f"attempt-{attempt}-{phase}-{uuid.uuid4().hex}.json"
        self.save(self.relative(evidence_path), result)
        reference = self._transaction_reference(result["output"]) or known_reference
        evidence = {"phase": phase, "result": self.relative(evidence_path), "transaction": reference}
        if phase == "proposal" and isinstance(result["output"], dict) and "proposal_id" in result["output"]:
            evidence["proposal_id"] = result["output"]["proposal_id"]
        self._event(stage, key, "submitted" if reference else "unknown", evidence, attempt=attempt)
        return result, reference

    def _record_proposal_identity(self, stage, key, attempt, submitted):
        matches = [event["evidence"]["proposal_id"] for event in self._operation_events(stage, key)
                   if event["attempt"] == attempt and event["event"] == "submitted"
                   and event["evidence"].get("phase") == "proposal" and "proposal_id" in event["evidence"]]
        require(all(value == submitted["proposal_id"] for value in matches), "proposal identity changed within attempt")
        if not matches:
            self._event(stage, key, "submitted", {"phase": "proposal", "proposal_id": submitted["proposal_id"],
                                                  "transaction": submitted["submission_ref"]}, attempt=attempt)

    def _proposal(self, stage, key, attempt, expected, proposal_id):
        deadline = time.monotonic() + (self.campaign["approval_wait_seconds"] if self.wait else 0)
        while True:
            proposal = self.read("dao.getProposal", {"dao_id": self.campaign["dao"], "id": proposal_id})
            require(proposal["id"] == proposal_id and proposal["proposer"] == self.campaign["submitter"]
                    and {name: proposal[name] for name in ("description", "kind")} == expected,
                    "DAO proposal ID/body/proposer differs from frozen request")
            status = proposal["status"]
            if status == "Approved":
                return proposal
            if status in ("Rejected", "Removed", "Expired", "Failed", "Moved"):
                self._event(stage, key, "failed", {"proposal_id": proposal_id, "proposal": proposal}, attempt=attempt)
                raise Stop(20, f"DAO proposal is {status}; no automatic replacement")
            require(status == "InProgress", "unknown DAO proposal status")
            if time.monotonic() >= deadline:
                self._event(stage, key, "waiting", {"proposal_id": proposal_id}, attempt=attempt)
                raise Stop(10, "waiting for external DAO approval")
            time.sleep(min(1, max(0, deadline - time.monotonic())))

    def _vote_receipts(self, reference, proposal_id, expected):
        result = self._tx(reference)
        require(any(receipt["executor_id"] == self.campaign["dao"] for receipt in result["receipts"]),
                "execution reference did not execute the DAO")
        # The canonical gateway supplies transaction actions where available;
        # never infer a vote ID from the latest proposal counter or a UI.
        transaction = result.get("transaction")
        require(isinstance(transaction, dict), "canonical tx.get transaction evidence required for final DAO vote")
        require(transaction["hash"] == reference["tx_hash"]
                and transaction["signer_id"] == reference["sender_account_id"]
                and transaction["receiver_id"] == self.campaign["dao"], "execution reference transaction identity mismatch")
        calls = transaction["function_calls"]
        require(transaction["action_count"] == 1 and len(calls) == 1 and calls[0]["index"] == 0,
                "execution reference is not exactly one DAO vote")
        call = calls[0]
        args = parse_json(base64.b64decode(call["args"], validate=True))
        require(call["method_name"] == "act_proposal" and args["id"] == proposal_id
                and args["action"] == "VoteApprove", "execution reference voted on another proposal/action")
        if args.get("proposal") is not None:
            require(args["proposal"] == expected["kind"], "final vote guards a different proposal kind")
        return result

    def ordinary(self, stage, key, plan_path, description, *, verify_execution, before_write=None):
        internal = JournalOperation(key, "dao")
        directory = self.operation_dir(stage, internal)
        plan_name = self.relative(plan_path)
        self.load(plan_name)
        binding = {"plan": plan_name, "plan_digest": self.artifacts[plan_name], "description": description}
        saved = directory / "request.json"
        if saved.exists():
            request = self.load(self.relative(saved))
            require(request["binding"] == binding, "ordinary operation source plan/description changed")
        else:
            planned = self.command(["dao", "propose", "--dao", self.campaign["dao"], "--plan", str(plan_path),
                                    "--description", description, "--signer-id", self.campaign["submitter"], "--print", "json"])
            require(planned["receiver_id"] == self.campaign["dao"] and len(planned["actions"]) == 1, "ordinary DAO preflight target differs")
            call = planned["actions"][0]["FunctionCall"]
            require(call["method_name"] == "add_proposal", "ordinary DAO preflight is not add_proposal")
            expected = parse_json(base64.b64decode(call["args"], validate=True))["proposal"]
            request = {"binding": binding, "proposal": expected, "attempt": 1, "preflight": planned}
            self.save(self.relative(saved), request)
            self.record(stage, internal, "prepared", request)
        attempt = request["attempt"]
        completed = self.confirmed(stage, internal)
        if completed:
            self._proposal(stage, internal, attempt, request["proposal"], completed["proposal_id"])
            verify_execution(self._vote_receipts(completed["execution_ref"], completed["proposal_id"], request["proposal"]))
            return completed
        submitted_path = directory / "submission.json"
        submitted = self.load(self.relative(submitted_path)) if submitted_path.exists() else None
        events = self._operation_events(stage, internal)
        started = [event for event in events if event["event"] == "attempt_started"]
        if submitted is None:
            if not started:
                if not self.submit:
                    raise Stop(10, "ordinary proposal prepared; --submit required")
                argv = ["dao", "propose", "--dao", self.campaign["dao"], "--plan", str(plan_path),
                        "--description", description, "--signer-id", self.campaign["submitter"], *self._signing_flags(self.campaign["submitter"])]
                result, reference = self._submit_once(stage, internal, attempt, "proposal", argv,
                                                       self.campaign["submitter"], before_write)
                output = result["output"]
            else:
                records = [event for event in events if event["event"] in ("submitted", "unknown") and event["evidence"].get("result")]
                output = self.load(records[-1]["evidence"]["result"])["output"] if records else None
                reference = self._transaction_reference(output)
            explicit = self._reference(stage, key, attempt)
            if reference:
                receipts = self._tx(reference)
                proposal_id = self._returned_id(receipts, self.campaign["dao"])
            elif explicit:
                proposal_id = explicit["dao_proposal_id"]
                self._proposal(stage, internal, attempt, request["proposal"], proposal_id)
                self._vote_receipts(explicit, proposal_id, request["proposal"])
                receipts = None
            else:
                self._event(stage, internal, "unknown", {"phase": "proposal", "reason": "transaction identity unavailable"}, attempt=attempt)
                raise Stop(30, "proposal may have been submitted; verified explicit execution reference required")
            if isinstance(output, dict) and "proposal_id" in output:
                require(output["proposal_id"] == proposal_id and output["proposal"] == request["proposal"], "native proposal result differs")
            submitted = {"proposal_id": proposal_id, "submission_ref": reference, "submission_receipts": receipts,
                         "native_output": output}
            self.save(self.relative(submitted_path), submitted)
        self._record_proposal_identity(stage, internal, attempt, submitted)
        proposal = self._proposal(stage, internal, attempt, request["proposal"], submitted["proposal_id"])
        reference = self._reference(stage, key, attempt, submitted["proposal_id"])
        if reference is None:
            raise Stop(10, "approved DAO proposal requires its explicit final-vote execution reference")
        receipts = self._vote_receipts(reference, submitted["proposal_id"], request["proposal"])
        verify_execution(receipts)
        result = {**submitted, "proposal": proposal, "execution_ref": reference, "receipts": receipts}
        self.confirm(stage, internal, result)
        return result

    def _signing_flags(self, signer, relayer=False):
        role = "RELAYER" if relayer else "SUBMITTER"
        return [] if os.environ.get(f"ASSET_MIGRATION_{role}_SECRET_KEY") or os.environ.get("SECRET_KEY") else ["--sign-with", "keychain"]

    def mpc(self, stage, key, market_record, plan_path, description, before_write=None):
        market = market_record
        internal = JournalOperation(key, "mpc")
        base_directory = self.operation_dir(stage, internal)
        history = self._operation_events(stage, internal)
        attempt = max((event["attempt"] for event in history), default=1)
        if history and history[-1]["event"] == "superseded":
            proof = history[-1]["evidence"]["review"]
            require(proof["expired"] and int(self.read("chain.getBlock", {})["height"]) > proof["last_valid_block"],
                    "superseded payload is not proven unusable on the current chain")
            self._lease(market, {"stage": stage, "key": key, "attempt": attempt},
                        release=True, proof={"expired": proof})
            attempt += 1
        directory = base_directory / f"attempt-{attempt}"
        directory.mkdir(parents=True, exist_ok=True)
        plan_name = self.relative(plan_path)
        self.load(plan_name)
        transport = self.campaign.get("mpc_transport", "delegate-action")
        binding = {"plan": plan_name, "plan_digest": self.artifacts[plan_name], "description": description,
                   "transport": transport, "account": market["account"], "public_key": market["public_key"],
                   "mpc_path": market["mpc_path"], "initial_nonce": market["initial_nonce"]}
        request_path = directory / "request.json"
        if request_path.exists():
            request = self.load(self.relative(request_path))
            require(request["binding"] == binding, "MPC source plan/key/path/transport changed")
        else:
            request = {"binding": binding, "attempt": attempt}
            self.save(self.relative(request_path), request)
            self._event(stage, internal, "prepared", request, attempt=attempt)
        attempt = request["attempt"]
        lease = {"stage": stage, "key": key, "attempt": attempt}
        completed = self.confirmed(stage, internal)
        if completed:
            self._proposal(stage, internal, attempt, completed["proposal_body"], completed["proposal_id"])
            signature_review, signature_receipts = self._mpc_signature_review(
                completed, completed["execution_ref"], market, transport, plan_path, description)
            receipts = self._tx(completed["transaction_ref"])
            self._verify_mpc_transaction(receipts, completed, market, transport)
            self._lease(market, lease, release=True,
                        proof={"transaction_ref": completed["transaction_ref"], "receipts": receipts})
            return {**completed, "signature_review": signature_review, "signature_receipts": signature_receipts}
        envelope = directory / "issued-envelope.json"
        submitted_path = directory / "submission.json"
        submitted = self.load(self.relative(submitted_path)) if submitted_path.exists() else None
        events = [event for event in self._operation_events(stage, internal) if event["attempt"] == attempt]
        proposal_started = [event for event in events if event["event"] == "attempt_started" and event["evidence"]["phase"] == "proposal"]
        if submitted is None:
            if not proposal_started:
                if not self.submit:
                    # A distinct disposable envelope is intentional: native
                    # propose writes --out before deciding whether to submit.
                    preflight = directory / f"preflight-envelope-{uuid.uuid4().hex}.json"
                    output = self.command(self._mpc_propose_args(plan_path, market, description, transport, preflight)
                                          + ["--print", "json"])
                    self.seal_tree(preflight)
                    self.save(self.relative(directory / f"preflight-{uuid.uuid4().hex}.json"), output)
                    raise Stop(10, "MPC proposal prepared; --submit required")
                require(not envelope.exists(), "issued MPC envelope exists without durable submission record; reconcile")
                argv = self._mpc_propose_args(plan_path, market, description, transport, envelope) + self._signing_flags(self.campaign["submitter"])
                result, reference = self._submit_once(stage, internal, attempt, "proposal", argv,
                                                       self.campaign["submitter"], before_write, envelope,
                                                       acquire_lease=(market, lease))
                output = result["output"]
            else:
                records = [event for event in events if event["event"] in ("submitted", "unknown") and event["evidence"].get("phase") == "proposal"
                           and event["evidence"].get("result")]
                output = self.load(records[-1]["evidence"]["result"])["output"] if records else None
                reference = self._transaction_reference(output)
            explicit = self._reference(stage, key, attempt)
            if reference:
                receipts = self._tx(reference)
                proposal_id = self._returned_id(receipts, self.campaign["dao"])
            elif explicit:
                proposal_id = explicit["dao_proposal_id"]
                receipts = None
            else:
                raise Stop(30, "MPC proposal outcome unknown; never issue another payload, supply verified execution reference")
            require(envelope.exists(), "issued MPC envelope lost; cannot reconstruct approved bytes")
            self.seal_tree(envelope)
            review = self.command(self._review_args("show", proposal_id, envelope)
                                  + ["--plan", str(plan_path), "--expected-description", description])
            self._check_review(review, market, transport, output)
            proposal = self.read("dao.getProposal", {"dao_id": self.campaign["dao"], "id": proposal_id})
            expected = {name: proposal[name] for name in ("description", "kind")}
            require(proposal["proposer"] == self.campaign["submitter"], "MPC proposal proposer mismatch")
            if explicit and not reference:
                self._vote_receipts(explicit, proposal_id, expected)
            submitted = {"proposal_id": proposal_id, "proposal_body": expected, "native_output": output,
                         "submission_ref": reference, "submission_receipts": receipts,
                         "payload": self.relative(envelope), "payload_digest": self.artifacts[self.relative(envelope)],
                         "review": review, "nonce": review["payload"]["nonce"],
                         "source_plan": plan_name, "source_artifact_digest": binding["plan_digest"],
                         "source_plan_digest": review["source_plan_digest"],
                         "source_plan_kind": review["source_plan_kind"],
                         "last_valid_block": review["last_valid_block"]}
            self.save(self.relative(submitted_path), submitted)
        self._record_proposal_identity(stage, internal, attempt, submitted)
        self._lease(market, lease)
        proposal = self._proposal(stage, internal, attempt, submitted["proposal_body"], submitted["proposal_id"])
        reference = self._reference(stage, key, attempt, submitted["proposal_id"])
        if reference is None:
            raise Stop(10, "MPC signature approval requires explicit final-vote execution reference")
        review, signature_receipts = self._mpc_signature_review(
            submitted, reference, market, transport, plan_path, description)
        transaction_path = directory / "transaction.json"
        execution_events = [event for event in events
                            if event["event"] == "attempt_started" and event["evidence"]["phase"] == "execute"]
        if transaction_path.exists():
            transaction = self.load(self.relative(transaction_path))
            transaction_ref = transaction["transaction_ref"]
        elif execution_events or reference.get("execution_tx_hash"):
            outcomes = [event for event in self._operation_events(stage, internal)
                        if event["attempt"] == attempt and event["event"] in ("submitted", "unknown")
                        and event["evidence"].get("phase") == "execute" and event["evidence"].get("result")]
            output = self.load(outcomes[-1]["evidence"]["result"])["output"] if outcomes else None
            transaction_ref = (self._transaction_reference(output)
                               or (execution_events[-1]["evidence"].get("known_transaction") if execution_events else None))
            if transaction_ref is None and reference.get("execution_tx_hash"):
                transaction_ref = {"tx_hash": reference["execution_tx_hash"],
                                   "sender_account_id": reference["execution_sender_account_id"]}
            if transaction_ref is None:
                raise Stop(30, "MPC execution outcome unknown and hash lost; explicit verified transaction evidence required")
            receipts = self._tx(transaction_ref)
            self._verify_mpc_transaction(receipts, submitted, market, transport)
            transaction = {"transaction_ref": transaction_ref, "native_output": output}
            self.save(self.relative(transaction_path), transaction)
            self._event(stage, internal, "submitted", {"phase": "execute", "transaction": transaction_ref,
                                                      "reconciled": True}, attempt=attempt)
        else:
            # The same native signature review above covers both fresh execution
            # and recovery; only a new write requires live nonce/expiry guards.
            require(not review["nonce_spent"] and review["key_installed"],
                    "MPC nonce spent/key changed; explicit execution reconciliation is required")
            if review["expired"]:
                self._event(stage, internal, "superseded", {"reason": "on-chain expiry", "review": review}, attempt=attempt)
                self._lease(market, lease, release=True, proof={"expired": review})
                raise Stop(20, "approved payload expired; retained attempt cannot be regenerated or executed")
            if not self.submit:
                raise Stop(10, "signature approved; execution still requires --submit")
            action = "relay" if transport == "delegate-action" else "broadcast"
            argv = self._review_args(action, submitted["proposal_id"], envelope) + ["--plan", str(plan_path),
                    "--tx-hash", reference["tx_hash"], "--tx-signer", reference["sender_account_id"]]
            signer = None
            if action == "relay":
                signer = self.campaign["relayer"]
                argv += ["--signer-id", signer, *self._signing_flags(signer, relayer=True)]
            known_reference = ({"tx_hash": encode_hash(submitted["review"]["payload_hash"]),
                                "sender_account_id": market["account"]} if transport == "transaction" else None)
            result, transaction_ref = self._submit_once(stage, internal, attempt, "execute", argv, signer,
                                                         before_write, known_reference=known_reference)
            if transaction_ref is None:
                raise Stop(30, "MPC execution identity missing; never resubmit")
            transaction = {"transaction_ref": transaction_ref, "native_output": result["output"]}
            self.save(self.relative(transaction_path), transaction)
        receipts = self._tx(transaction_ref)
        self._verify_mpc_transaction(receipts, submitted, market, transport)
        result = {**submitted, **transaction, "proposal": proposal, "execution_ref": reference,
                  "signature_review": review, "signature_receipts": signature_receipts, "receipts": receipts}
        self._event(stage, internal, "confirmed", result, attempt=attempt)
        self._lease(market, lease, release=True, proof={"transaction_ref": transaction_ref, "receipts": receipts})
        return result

    def _mpc_signature_review(self, submitted, reference, market, transport, plan_path, description):
        receipts = self._vote_receipts(reference, submitted["proposal_id"], submitted["proposal_body"])
        self.load(submitted["payload"])
        review = self.command(self._review_args("show", submitted["proposal_id"], self.path(submitted["payload"]))
                              + ["--plan", str(plan_path), "--expected-description", description,
                                 "--tx-hash", reference["tx_hash"], "--tx-signer", reference["sender_account_id"]])
        self._check_review(review, market, transport, submitted["native_output"])
        require(all(review[name] == submitted["review"][name] for name in
                    ("payload", "payload_hash", "source_plan_kind", "source_plan_digest")),
                "verified signature review differs from the frozen MPC request")
        # Native show uses the same cryptographic extraction as relay/broadcast;
        # a successful nonexecuting first vote cannot supply this receipt proof.
        proof = review["signature_execution"]
        index = proof["receipt_index"]
        require(proof["tx_hash"] == reference["tx_hash"]
                and proof["sender_account_id"] == reference["sender_account_id"]
                and type(index) is int and 0 <= index < len(receipts["receipts"])
                and proof["receipt"] == receipts["receipts"][index],
                "native MPC signature proof does not match the exact final-vote receipt graph")
        return review, receipts

    def _mpc_propose_args(self, plan, market, description, transport, envelope):
        argv = ["dao", "mpc", "propose", "--dao", self.campaign["dao"], "--mpc-contract", self.campaign["mpc_contract"],
                "--key-type", self.campaign["mpc_key_type"], "--path", market["mpc_path"], "--plan", str(plan),
                "--kind", transport, "--description", description, "--out", str(envelope), "--blind",
                "--signer-id", self.campaign["submitter"]]
        if self.campaign.get("mpc_valid_for_blocks") is not None:
            argv += ["--valid-for-blocks", str(self.campaign["mpc_valid_for_blocks"])]
        return argv

    def _review_args(self, action, proposal_id, envelope):
        return ["dao", "mpc", action, "--dao", self.campaign["dao"], "--proposal-id", str(proposal_id),
                "--mpc-contract", self.campaign["mpc_contract"], "--payload-file", str(envelope)]

    def _check_review(self, review, market, transport, native):
        require(review["dao"] == self.campaign["dao"] and review["mpc_contract_id"] == self.campaign["mpc_contract"]
                and review["proposer"] == self.campaign["submitter"] and review["path"] == market["mpc_path"]
                and review["derived_public_key"] == market["public_key"]
                and review["payload"]["signer_id"] == market["account"]
                and review["payload"]["kind"] == transport.replace("-", "_"), "native MPC review identity/path/transport mismatch")
        if isinstance(native, dict) and "payload_hash" in native:
            require(review["payload_hash"] == native["payload_hash"] and review["payload"]["nonce"] == native["nonce"],
                    "MPC issued payload hash/nonce changed")

    def _verify_mpc_transaction(self, result, submitted, market, transport):
        verify_receipts(result)
        transaction = result.get("transaction")
        require(isinstance(transaction, dict), "MPC execution has no canonical transaction proof")
        expected = hash_bytes(submitted["review"]["payload_hash"])
        if transport == "transaction":
            require(hash_bytes(transaction["hash"]) == expected
                    and transaction["signer_id"] == market["account"],
                    "raw transaction hash/sender does not match approved MPC payload")
        else:
            delegates = transaction.get("delegate_actions", [])
            require(transaction["receiver_id"] == market["account"]
                    and transaction["action_count"] == 1 and len(delegates) == 1
                    and delegates[0]["index"] == 0
                    and hash_bytes(delegates[0]["payload_hash"]) == expected,
                    "delegate execution does not carry exactly the approved MPC payload")

    def load_snapshot(self, relative):
        return self.parse_snapshot(relative)

    def require_paused(self):
        from governance import cached_price_request, certify_pause, context_from_reads, context_requests, feed_key, feeds
        paused = self.confirmed("02-pause", "stage")
        require(paused is not None, "cohort pause stage is not confirmed")
        certify_pause(self, paused)
        grouped = {}
        for feed in feeds(self):
            grouped.setdefault(feed["oracle"], []).append(feed)
        require(set(grouped) == set(paused["oracle_contexts"]), "paused oracle cohort changed")
        contexts, observations = {}, []
        for oracle, expected in paused["oracle_contexts"].items():
            requests = [("account.get", {"account_id": oracle})]
            groups = []
            for feed in grouped[oracle]:
                queries = context_requests(self, feed)
                groups.append((feed, len(requests), len(queries)))
                requests.extend(queries)
                requests.append(cached_price_request(self, feed))
            storage, _snapshot, values = self.snapshot_with_reads(
                oracle, f"observations/{uuid.uuid4().hex}/oracle", requests)
            actual = {"identity": {name: values[0].get(name) for name in
                                  ("code_hash", "global_contract_hash", "global_contract_account_id")},
                      "storage_digest": self.storage_digest(storage), "storage_entries": len(storage)}
            actual["effective_code_sha256"] = self.snapshot_code_hash(_snapshot)
            require(actual == expected, "paused oracle identity/raw-storage context changed")
            contexts[oracle] = actual
            for feed, offset, width in groups:
                live = context_from_reads(feed, values[offset:offset + width])
                require(live["breaker"]["is_manually_tripped"], f"cohort feed is not paused: {feed_key(feed)}")
                require(values[offset + width] is None, f"paused feed exposes cached price: {feed_key(feed)}")
                observations.append({"key": feed_key(feed), "context": live})
        return {"feeds": observations, "oracle_contexts": contexts}

    def assert_no_unresolved(self):
        latest = {}
        for event in self.events:
            if event["operation"] is not None:
                latest[(event["stage"], event["key"], event["operation"], event["attempt"])] = event
        for event in latest.values():
            if event["event"] in ("attempt_started", "submitted", "unknown", "waiting", "failed"):
                raise Stop(30 if event["event"] in ("attempt_started", "submitted", "unknown") else 20,
                           f"unresolved operation: {event['stage']} {event['key']}")
        for market in self.intent["markets"]:
            require(self._lease(market, {}, inspect=True) is None,
                    "derived key still has a durable outstanding payload lease")

    def recheck_metadata(self):
        expected = [
            pair for market in self.intent["markets"]
            for pair in frozen_market_metadata(market)
        ] + [
            (asset["reference"], asset["metadata"])
            for consumer in self.intent.get("dependency_consumers", [])
            for asset in consumer["assets"]
        ]
        actual = self.read_many([
            ("token.getMetadata", {"token": reference}) for reference, _metadata in expected
        ])
        require(all(value == metadata for value, (_reference, metadata) in zip(actual, expected)),
                "frozen campaign or dependency asset metadata drift")

    def recheck_progress(self, complete=False):
        """Revalidate scoped state and inventories, applying only confirmed swap deltas."""
        self._assert_no_ambiguous_submission()
        self.recheck_metadata()
        report = {"markets": [], "complete": complete}
        for market in self.intent["markets"]:
            account = market["account"]
            patch = self.confirmed("03-patch", account)
            if complete:
                require(patch is not None, f"missing confirmed patch: {account}")
            expected_configuration = market["expected_configuration"] if patch else market["configuration"]
            baseline = patch["baseline"] if patch else market["snapshot"]
            expected_storage = self.parse_snapshot(baseline, account)
            expected_balances = {}
            references = {}
            inventories = {}
            for asset in market["unchanged_assets"]:
                reference = asset_reference(asset["reference"])
                name = canonical(reference).decode()
                expected_balances[name] = int(asset["balance"])
                references[name] = reference
            for role in market["roles"]:
                inventory = role.get("inventory_role", role["role"])
                if inventory in inventories:
                    other = inventories[inventory]
                    require(all(role[name] == other[name] for name in ("old_reference", "new_reference", "initial_old_balance", "initial_new_balance")),
                            "incompatible duplicate inventory")
                    continue
                inventories[inventory] = role
                for side in ("old", "new"):
                    reference = asset_reference(role[f"{side}_reference"])
                    name = canonical(reference).decode()
                    balance = int(role[f"initial_{side}_balance"])
                    require(name not in expected_balances or expected_balances[name] == balance, "conflicting initial balance references")
                    expected_balances[name] = balance
                    references[name] = reference
            settlements = []
            if patch:
                self._recheck_receipt_evidence(patch)
            for inventory, role in inventories.items():
                swap = self.confirmed("04-swap", f"{account}:{inventory}")
                if complete:
                    require(swap is not None, f"missing confirmed inventory settlement: {account}:{inventory}")
                if swap is None:
                    continue
                require(patch is not None, "swap recorded without confirmed config patch")
                self._recheck_receipt_evidence(swap, allow_zero=swap.get("zero_inventory") is True)
                amount = int(swap["amount"])
                require(amount >= 0 and int(swap["old_before"]) == amount
                        and int(swap["old_after"]) == 0
                        and int(swap["old_before"]) == int(role["initial_old_balance"])
                        and int(swap["new_after"]) - int(swap["new_before"]) == amount,
                        "recorded swap violates exact raw-unit conservation")
                require(not swap.get("zero_inventory") or amount == 0, "zero-inventory result has nonzero amount")
                old = canonical(asset_reference(role["old_reference"])).decode()
                new = canonical(asset_reference(role["new_reference"])).decode()
                expected_balances[old] -= amount
                expected_balances[new] += amount
                require(expected_balances[old] >= 0, "settlement consumes more than frozen inventory")
                settlements.append({"inventory_role": inventory, "evidence": swap})
            requests = [
                ("market.getConfiguration", {"market_id": account}),
                ("account.get", {"account_id": account}),
            ] + [
                ("token.getBalanceOf", {"token": references[name], "account_id": account})
                for name in expected_balances
            ]
            storage, actual_snapshot, values = self.snapshot_with_reads(
                account, f"observations/{uuid.uuid4().hex}/market", requests)
            configuration, native = values[:2]
            balance_values = values[2:]
            require(configuration == expected_configuration, f"market configuration drift: {account}")
            identity = {name: native.get(name) for name in ("code_hash", "global_contract_hash", "global_contract_account_id")}
            require(identity == market["account_identity"], f"market code/global linkage drift: {account}")
            require(storage == expected_storage, f"raw market storage drift/new deposit credit: {account}")
            balances = []
            for (name, expected), value in zip(expected_balances.items(), balance_values):
                actual = token_amount(value)
                require(actual == expected, f"unexplained asset balance/refund drift: {account} {name}")
                balances.append({"token": references[name], "expected": str(expected), "actual": str(actual)})
            access_key = self._installed_key(market)
            report["markets"].append({"account": account, "configuration": configuration, "identity": identity,
                                      "baseline": baseline, "snapshot": actual_snapshot,
                                      "storage_digest": self.storage_digest(storage), "balances": balances,
                                      "settlements": settlements, "patch": patch,
                                      "native_account": native, "access_key": access_key})
        report["dependency_consumers"] = []
        for consumer in self.intent.get("dependency_consumers", []):
            account = consumer["account"]
            configuration = self.read("market.getConfiguration", {"market_id": account})
            require(configuration == consumer["configuration"], f"shared dependency configuration drift: {account}")
            identity = self.identity(account)
            require(identity == consumer["account_identity"], f"shared dependency code/linkage drift: {account}")
            storage, snapshot = self.snapshot(account, f"observations/{uuid.uuid4().hex}/dependency")
            require(storage == self.parse_snapshot(consumer["snapshot"], account),
                    f"shared dependency raw-storage drift: {account}")
            assets = []
            for asset in consumer["assets"]:
                balance = self.balance(asset["reference"], account)
                require(balance == int(asset["balance"]), f"shared dependency asset balance drift: {account}")
                assets.append({"reference": asset["reference"], "balance": str(balance)})
            report["dependency_consumers"].append({"account": account, "configuration": configuration,
                                                   "identity": identity, "snapshot": snapshot,
                                                   "storage_digest": self.storage_digest(storage), "assets": assets})
        report["registrations"] = self._registration_observations(required=complete)
        return report

    def _recheck_receipt_evidence(self, evidence, allow_zero=False):
        receipts = evidence.get("receipts")
        if allow_zero and receipts is None:
            return
        verify_receipts(receipts)
        reference = evidence.get("transaction_ref")
        if reference:
            self._tx(reference)
        if isinstance(evidence.get("mpc"), dict):
            self._recheck_receipt_evidence(evidence["mpc"])

    def _registration_observations(self, required=False):
        required = required or self.confirmed("01-register", "stage") is not None
        result = []
        registrations = self.intent.get("registrations", [])
        balances = self.read_many([
            ("storage.getBalanceOf", {name: registration[name] for name in ("contract_id", "account_id")})
            for registration in registrations
        ])
        for registration, balance in zip(registrations, balances):
            params = {name: registration[name] for name in ("contract_id", "account_id")}
            if required or registration["initial_balance"] is not None:
                require(registration_funded(registration, balance),
                        "required destination/converter registration is missing or underfunded")
            result.append({**params, "minimum": registration["minimum"], "balance": balance})
        return result

    def _assert_no_ambiguous_submission(self):
        phases = {}
        for event in self.events:
            if event["operation"] is None:
                continue
            phase = event["evidence"].get("phase")
            identity = (event["stage"], event["key"], event["operation"], event["attempt"])
            if phase:
                phases[(*identity, phase)] = event["event"]
            elif event["event"] in ("confirmed", "superseded"):
                for key in list(phases):
                    if key[:4] == identity:
                        phases[key] = event["event"]
        if any(event in ("attempt_started", "unknown") for event in phases.values()):
            raise Stop(30, "ambiguous financial submission must be reconciled before continuing")

    def recheck_reconciliation(self):
        self.assert_no_unresolved()
        return self.recheck_progress(complete=True)


STAGES = ("00-prepare", "01-register", "02-pause", "03-patch", "04-swap", "05-reconcile", "06-resume", "07-finalize")


def _parser(preparation=False):
    parser = argparse.ArgumentParser()
    parser.add_argument("--run-dir", required=True)
    parser.add_argument("--submit", action="store_true")
    parser.add_argument("--wait", action="store_true")
    parser.add_argument("--execution-refs")
    if preparation:
        parser.add_argument("--campaign")
        parser.add_argument("--market")
        parser.add_argument("--borrow-asset")
        parser.add_argument("--collateral-asset")
    return parser


def stage_main(stage, handler):
    run = None
    try:
        args = _parser(stage == "00-prepare").parse_args()
        run = Run(args, stage)
        if run.intent:
            identity = run.read("chain.getIdentity", None)
            require(identity == {"chain_id": run.campaign["expected_chain_id"],
                                 "genesis_hash": run.campaign["expected_genesis_hash"]},
                    "frozen campaign chain identity changed")
        result = handler(run)
        if result is not None:
            print(canonical({"stage": stage, "status": "confirmed", "result": result}).decode())
        raise SystemExit(0)
    except Stop as error:
        print(canonical({"stage": stage, "code": error.code, "message": error.message}).decode())
        raise SystemExit(error.code)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(canonical({"stage": stage, "code": 20, "message": str(error)}).decode())
        raise SystemExit(20)
    finally:
        if run is not None:
            run.close()


def runner_main():
    """Resume the fixed sequence at its earliest unconfirmed stage."""
    parser = _parser(True)
    args = parser.parse_args(sys.argv[2:] if sys.argv[1:2] == ["run"] else None)
    run = None
    try:
        if (Path(args.run_dir) / "intent.json").exists():
            run = Run(args, "00-prepare")
            if args.campaign is not None:
                require(normalize_campaign(args) == run.campaign, "authored campaign changed after intent freeze")
            else:
                require(not any(getattr(args, name) is not None for name in ("market", "borrow_asset", "collateral_asset")),
                        "market overrides require the original campaign input")
            start = next((index for index, stage in enumerate(STAGES) if run.confirmed(stage, "stage") is None), 7)
        else:
            start = 0
    except Stop as error:
        print(canonical({"code": error.code, "message": error.message}).decode())
        return error.code
    finally:
        if run is not None:
            run.close()
    common = ["--run-dir", args.run_dir]
    if args.submit:
        common.append("--submit")
    if args.wait:
        common.append("--wait")
    if args.execution_refs:
        common += ["--execution-refs", args.execution_refs]
    for index in range(start, len(STAGES)):
        argv = [sys.executable, str(Path(__file__).with_name(STAGES[index] + ".py")), *common]
        if index == 0:
            for name in ("campaign", "market", "borrow_asset", "collateral_asset"):
                if getattr(args, name):
                    argv += ["--" + name.replace("_", "-"), getattr(args, name)]
        result = subprocess.run(argv, check=False)
        if result.returncode != 0:
            return result.returncode
    return 0


if __name__ == "__main__":
    sys.exit(runner_main())
