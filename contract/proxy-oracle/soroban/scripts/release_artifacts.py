"""Catalog and shared release helpers for proxy-oracle Soroban artifacts."""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterator

@dataclass(frozen=True)
class Artifact:
    slug: str
    package: str
    max_optimized_size: int
    spec_policy: str

    @property
    def wasm(self) -> str:
        return f"{self.package.replace('-', '_')}.wasm"

    @property
    def optimized_wasm(self) -> str:
        return f"{self.package.replace('-', '_')}.optimized.wasm"

    @property
    def manifest_path(self) -> str:
        return f"target/proxy-oracle-soroban/wasm/{self.optimized_wasm}"


ARTIFACTS = (
    Artifact("runtime", "templar-proxy-oracle-soroban-contract", 131_072, "runtime_v1"),
    Artifact(
        "governance",
        "templar-proxy-oracle-soroban-governance-contract",
        131_072,
        "governance_v1",
    ),
    Artifact(
        "sep40_adapter",
        "templar-proxy-oracle-soroban-sep40-adapter-contract",
        32_768,
        "sep40_adapter_v1",
    ),
    Artifact(
        "lazer_source",
        "templar-proxy-oracle-soroban-pyth-lazer-source-contract",
        32_768,
        "lazer_source_v1",
    ),
    Artifact(
        "batcher",
        "templar-proxy-oracle-soroban-batcher-contract",
        32_768,
        "ownerless_batcher_v1",
    ),
)

ROOT = Path(__file__).resolve().parents[4]
# Linux exposes open descriptors here; macOS uses /dev/fd.
FD_DIRECTORY = "/proc/self/fd" if Path("/proc/self/fd").is_dir() else "/dev/fd"

RELEASE_DIR = ROOT / "target/proxy-oracle-soroban"
WASM_DIR = RELEASE_DIR / "wasm"
MANIFEST_PATH = RELEASE_DIR / "release-manifest.json"
EVIDENCE_PATH = RELEASE_DIR / "evidence/artifact-validation.txt"
LOCK_PATH = RELEASE_DIR / ".release.lock"
SHA256 = re.compile(r"^[0-9a-f]{64}$")
STELLAR_VERSION = re.compile(r"^stellar\s+(\d+\.\d+\.\d+)(?:\s|$)")


def fail(message: str) -> None:
    raise ValueError(message)

def _is_ascii_digits(value: str) -> bool:
    return bool(value) and all("0" <= character <= "9" for character in value)


def _is_semver_number(value: str) -> bool:
    return _is_ascii_digits(value) and (
        value == "0" or not value.startswith("0")
    )


def _has_valid_semver_identifiers(
    value: str, *, allow_numeric_leading_zero: bool
) -> bool:
    for identifier in value.split("."):
        if not identifier or not all(
            character.isascii()
            and (character.isalnum() or character == "-")
            for character in identifier
        ):
            return False
        if (
            not allow_numeric_leading_zero
            and _is_ascii_digits(identifier)
            and not _is_semver_number(identifier)
        ):
            return False
    return True


def is_semver(value: str) -> bool:
    release_and_prerelease, build_separator, build = value.partition("+")
    if build_separator and (
        "+" in build
        or not _has_valid_semver_identifiers(
            build, allow_numeric_leading_zero=True
        )
    ):
        return False
    release, prerelease_separator, prerelease = (
        release_and_prerelease.partition("-")
    )
    numbers = release.split(".")
    if len(numbers) != 3 or not all(
        _is_semver_number(number) for number in numbers
    ):
        return False
    return not prerelease_separator or _has_valid_semver_identifiers(
        prerelease, allow_numeric_leading_zero=False
    )



def canonical_json(value: object) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode()


def reject_duplicate_object_keys(
    pairs: list[tuple[str, object]],
) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, entry in pairs:
        if key in value:
            fail(f"duplicate JSON key: {key}")
        value[key] = entry
    return value


def parse_json(text: str) -> object:
    return json.loads(
        text,
        object_pairs_hook=reject_duplicate_object_keys,
        parse_constant=lambda value: fail(f"invalid JSON constant: {value}"),
    )


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(65_536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def stable_stellar_version(output: str) -> str:
    output = output.strip()
    if not output or len(output) > 8_192:
        fail("Stellar CLI output is empty or oversized")
    for line in output.splitlines():
        match = STELLAR_VERSION.match(line)
        if match is None:
            continue
        version = tuple(map(int, match.group(1).split(".")))
        if (25, 2, 0) <= version < (27, 0, 0):
            return match.group(1)
        fail(f"unsupported Stellar CLI version: {match.group(1)}")
    fail("cannot parse stable Stellar CLI version")


def run_text(
    args: list[str],
    *,
    cwd: Path = ROOT,
    env: dict[str, str] | None = None,
    pass_fds: tuple[int, ...] = (),
) -> str:
    result = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        pass_fds=pass_fds,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip()
        fail(f"{' '.join(args)} failed: {detail}")
    return result.stdout.strip()


def stellar_version_metadata() -> tuple[str, str]:
    output = run_text(["stellar", "--version"])
    return stable_stellar_version(output), output


def package_versions(root: Path = ROOT) -> dict[str, str]:
    metadata = parse_json(
        run_text(
            ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
            cwd=root,
        )
    )
    if not isinstance(metadata, dict) or not isinstance(metadata.get("packages"), list):
        fail("invalid locked Cargo metadata")
    versions: dict[str, str] = {}
    for package in metadata["packages"]:
        if (
            not isinstance(package, dict)
            or not isinstance(package.get("name"), str)
            or not isinstance(package.get("version"), str)
        ):
            fail("invalid package in locked Cargo metadata")
        versions[package["name"]] = package["version"]
    missing = {artifact.package for artifact in ARTIFACTS} - versions.keys()
    if missing:
        fail(f"release packages absent from locked Cargo metadata: {sorted(missing)}")
    return versions


@contextlib.contextmanager
def release_lock() -> Iterator[None]:
    if RELEASE_DIR.is_symlink():
        fail("release directory must not be a symlink")
    RELEASE_DIR.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(
        LOCK_PATH,
        os.O_RDWR
        | os.O_CREAT
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_CLOEXEC", 0),
        0o600,
    )
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
            fail("release lock must be a single-link regular file")
        os.fchmod(descriptor, 0o600)
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
    finally:
        os.close(descriptor)


def catalog_artifact(slug: str) -> Artifact:
    for artifact in ARTIFACTS:
        if artifact.slug == slug:
            return artifact
    fail(f"unknown artifact slug: {slug}")


def selected_artifacts(slug: str) -> tuple[Artifact, ...]:
    return ARTIFACTS if slug == "all" else (catalog_artifact(slug),)


def sizes_tsv() -> str:
    rows = []
    for artifact in ARTIFACTS:
        path = WASM_DIR / artifact.optimized_wasm
        try:
            size = str(path.stat().st_size)
        except FileNotFoundError:
            size = "missing"
        rows.append("\t".join((artifact.slug, size, str(path))))
    return "\n".join(rows)


def catalog_tsv() -> str:
    return "\n".join(
        "\t".join(
            (
                artifact.slug,
                artifact.package,
                artifact.optimized_wasm,
                str(artifact.max_optimized_size),
                artifact.spec_policy,
            )
        )
        for artifact in ARTIFACTS
    )


def _udt(name: str) -> dict[str, dict[str, str]]:
    return {"udt": {"name": name}}


def _vec(element_type: object) -> dict[str, dict[str, object]]:
    return {"vec": {"element_type": element_type}}


def _option(value_type: object) -> dict[str, dict[str, object]]:
    return {"option": {"value_type": value_type}}


def _result(ok_type: object, error: str) -> dict[str, dict[str, object]]:
    return {"result": {"ok_type": ok_type, "error_type": _udt(error)}}


def _function(
    inputs: list[tuple[str, object]], outputs: list[object]
) -> dict[str, object]:
    return {
        "inputs": [{"name": name, "type_": type_} for name, type_ in inputs],
        "outputs": outputs,
    }


ASSET = _udt("Asset")
PRICE_DATA = _udt("PriceData")
def _case(name: str, *types: object) -> dict[str, object]:
    return {"name": name, "type_": list(types)}


def _codes(*names: str) -> list[dict[str, object]]:
    return [
        {"name": name, "value": value}
        for value, name in enumerate(names, start=1)
    ]


CONTRACT_ERROR_CODES = _codes(
    "AlreadyInitialized",
    "MissingConfig",
    "Unauthorized",
    "InvalidInput",
    "StorageError",
    "SourceUnavailable",
    "ResolveFailed",
    "ConversionOverflow",
    "TooManySources",
    "TooManyBreakers",
    "BreakerError",
    "TooManyAssets",
    "TooFewSources",
)
LAZER_SOURCE_ERROR_CODES = _codes(
    "InvalidInput",
    "Unauthorized",
    "InvalidPayload",
    "ChannelMismatch",
    "ArithmeticOverflow",
    "VerifierRejected",
    "VerifierInvokeFailed",
)
GOVERNANCE_FUNCTION_NAMES = (
    "__constructor",
    "next_proposal_id",
    "get_proposal",
    "get_effective_proposal_ttl",
    "get_operation_ttl",
    "create_proposal",
    "cancel_proposal",
    "execute_proposal",
    "has_role",
    "list_role",
    "get_roles",
    "active_ids",
    "proxy_oracle",
    "extend_ttl",
)
GOVERNANCE_ACTION_CASES = [
    _case("SetProxy", _udt("Asset"), _udt("ProxyConfig")),
    _case("RemoveProxy", _udt("Asset")),
    _case("ConfigureBreakers", _udt("Asset"), "u64", "u32"),
    _case("AddBreaker", _udt("Asset"), _udt("CircuitBreakerConfig")),
    _case("RemoveBreaker", _udt("Asset"), "u32"),
    _case("Rearm", _udt("Asset"), "u32", _udt("RearmConfig")),
    _case("SetEnforced", _udt("Asset"), "u32", _udt("SetEnforcedConfig")),
    _case(
        "SetManualTrip",
        _udt("Asset"),
        "bool",
        {"option": {"value_type": "bytes"}},
    ),
    _case("TransferOwnership", "address"),
    _case("AcceptOwnership"),
    _case("RenounceOwnership"),
    _case("SetActionTtl", _udt("OperationKind"), "u64"),
    _case("SetRole", "address", _udt("Role"), "bool"),
    _case("Upgrade", {"bytes_n": {"n": 32}}),
]
GOVERNANCE_ERROR_CODES = _codes(
    "AlreadyInitialized",
    "Unauthorized",
    "MissingConfig",
    "ProposalNotFound",
    "ProposalNotMature",
    "ArithmeticOverflow",
    "RuntimeFailed",
    "ProposalOutOfOrder",
    "InvalidInput",
    "TtlExceedsMaximum",
    "LastAdmin",
)


ASSET_CASES = [_case("Stellar", "address"), _case("Other", "symbol")]
REFRESH_STATUS_CASES = [
    _case("Accepted", _udt("NormalizedPrice")),
    _case("Blocked", "u32"),
    _case("ResolveFailed", "u32"),
    _case("UnknownAsset"),
    _case("SourceUnavailable"),
]
SOURCE_CONFIG_FIELDS = [
    {"name": "asset", "type_": ASSET},
    {"name": "oracle", "type_": "address"},
]
PROXY_CONFIG_FIELDS = [
    {"name": "max_age_secs", "type_": _option("u64")},
    {"name": "max_clock_drift_secs", "type_": _option("u64")},
    {"name": "min_sources", "type_": "u32"},
    {"name": "sources", "type_": _vec(_udt("SourceConfig"))},
]


@dataclass(frozen=True)
class AbiPolicy:
    functions: dict[str, dict[str, object]]
    structs: dict[str, list[dict[str, object]]] = field(default_factory=dict)
    # Case order is the wire encoding, so pin the whole ordered list.
    unions: dict[str, list[dict[str, object]]] = field(default_factory=dict)
    # Discriminants are on-wire failure codes.
    error_enums: dict[str, list[dict[str, object]]] = field(
        default_factory=dict
    )
    required: frozenset[str] = frozenset()
    forbidden: frozenset[str] = frozenset()
    exact_function_names: frozenset[str] | None = None


SEP40_FUNCTIONS = {
    "assets": _function([], [_vec(ASSET)]),
    "decimals": _function([], ["u32"]),
    "resolution": _function([], ["u32"]),
    "price": _function(
        [("asset", ASSET), ("timestamp", "u64")],
        [_option(PRICE_DATA)],
    ),
    "prices": _function(
        [("asset", ASSET), ("records", "u32")],
        [_option(_vec(PRICE_DATA))],
    ),
    "lastprice": _function(
        [("asset", ASSET)], [_option(PRICE_DATA)]
    ),
}
# Losing these leaves a contract ungovernable and unupgradeable, so every
# owner-governed policy requires them; the batcher forbids the whole surface.
OWNERSHIP_EXPORTS = frozenset(
    {
        "get_owner",
        "transfer_ownership",
        "accept_ownership",
        "upgrade",
    }
)


def _upgrade(error: str) -> dict[str, dict[str, object]]:
    return {
        "upgrade": _function(
            [
                ("new_wasm_hash", {"bytes_n": {"n": 32}}),
                ("operator", "address"),
            ],
            [_result("void", error)],
        )
    }


BATCHER_FUNCTIONS = {
    "refresh_many": _function(
        [("oracle", "address"), ("assets", _vec(ASSET))],
        [_vec(_udt("RefreshStatus"))],
    ),
    "extend_ttl_many": _function(
        [("oracle", "address"), ("assets", _vec(ASSET))],
        [_vec("bool")],
    ),
    "extend_ttl_contracts": _function(
        [("contracts", _vec("address"))], [_vec("bool")]
    ),
}
ABI_POLICIES = {
    "runtime_v1": AbiPolicy(
        functions={
            "__constructor": _function(
                [("governance", "address"), ("base", ASSET)], []
            ),
            "refresh": _function(
                [("asset", ASSET)], [_udt("RefreshStatus")]
            ),
            **_upgrade("ContractError"),
        },
        structs={
            "SourceConfig": SOURCE_CONFIG_FIELDS,
            "ProxyConfig": PROXY_CONFIG_FIELDS,
        },
        unions={"Asset": ASSET_CASES, "RefreshStatus": REFRESH_STATUS_CASES},
        error_enums={"ContractError": CONTRACT_ERROR_CODES},
        required=OWNERSHIP_EXPORTS,
    ),
    "governance_v1": AbiPolicy(
        functions={
            "__constructor": _function(
                [
                    ("admin", "address"),
                    ("proxy_oracle", "address"),
                    ("initial_uniform_ttl_ns", "u64"),
                ],
                [_result("void", "GovernanceError")],
            ),
        },
        structs={
            "SourceConfig": SOURCE_CONFIG_FIELDS,
            "ProxyConfig": PROXY_CONFIG_FIELDS,
        },
        unions={"GovernanceAction": GOVERNANCE_ACTION_CASES},
        error_enums={"GovernanceError": GOVERNANCE_ERROR_CODES},
        # Role-governed, so an ownership surface here would be a second,
        # ungoverned path to every runtime config change.
        forbidden=OWNERSHIP_EXPORTS | frozenset({"renounce_ownership"}),
        exact_function_names=frozenset(GOVERNANCE_FUNCTION_NAMES),
    ),
    "sep40_adapter_v1": AbiPolicy(
        functions={
            "__constructor": _function(
                [
                    ("owner", "address"),
                    ("parent_oracle", "address"),
                    ("asset", ASSET),
                    ("decimals", "u32"),
                    ("resolution", "u32"),
                    ("base", ASSET),
                ],
                [_result("void", "ContractError")],
            ),
            "set_decimals": _function(
                [("decimals", "u32")], [_result("void", "ContractError")]
            ),
            **_upgrade("ContractError"),
            **SEP40_FUNCTIONS,
        },
        unions={"Asset": ASSET_CASES},
        required=OWNERSHIP_EXPORTS,
    ),
    "lazer_source_v1": AbiPolicy(
        functions={
            "__constructor": _function(
                [
                    ("owner", "address"),
                    ("config", _udt("Config")),
                    ("supported_feed_ids", _vec("u32")),
                ],
                [_result("void", "LazerSourceError")],
            ),
            "update_price_feeds": _function(
                [("payload", "bytes")], [_result("u32", "LazerSourceError")]
            ),
            "set_supported_feed_ids": _function(
                [("supported_feed_ids", _vec("u32"))],
                [_result("void", "LazerSourceError")],
            ),
            "set_verification_config": _function(
                [
                    ("verifier", "address"),
                    ("channel", _udt("LazerChannel")),
                ],
                [_result("void", "LazerSourceError")],
            ),
            "reset_verification_epoch": _function(
                [], [_result("void", "LazerSourceError")]
            ),
            "set_freshness": _function(
                [("freshness", _udt("FreshnessConfig"))],
                [_result("void", "LazerSourceError")],
            ),
            **_upgrade("LazerSourceError"),
            **SEP40_FUNCTIONS,
        },
        structs={
            "FreshnessConfig": [
                {"name": "max_age_secs", "type_": "u64"},
                {"name": "max_clock_drift_secs", "type_": "u64"},
            ],
            "Config": [
                {"name": "base", "type_": ASSET},
                {"name": "channel", "type_": _udt("LazerChannel")},
                {"name": "decimals", "type_": "u32"},
                {"name": "freshness", "type_": _udt("FreshnessConfig")},
                {"name": "verifier", "type_": "address"},
            ],
        },
        unions={
            "Asset": ASSET_CASES,
            "LazerChannel": [
                _case("RealTime"),
                _case("FixedRate50ms"),
                _case("FixedRate200ms"),
                _case("FixedRate1000ms"),
            ],
        },
        error_enums={"LazerSourceError": LAZER_SOURCE_ERROR_CODES},
        required=OWNERSHIP_EXPORTS,
        # `MockVerifier` lives behind the `testutils` feature in this crate and
        # two `#[contractimpl]`s in one cdylib share a wasm, so a release build
        # that ever enables it would export these.
        forbidden=frozenset(
            {"set_decimals", "verify_update", "set_reject", "set_trap"}
        ),
    ),
    "ownerless_batcher_v1": AbiPolicy(
        functions=BATCHER_FUNCTIONS,
        unions={"Asset": ASSET_CASES, "RefreshStatus": REFRESH_STATUS_CASES},
        forbidden=OWNERSHIP_EXPORTS | frozenset({"renounce_ownership"}),
        exact_function_names=frozenset(BATCHER_FUNCTIONS),
    ),
}


def normalize_function(function: dict[str, Any]) -> dict[str, object]:
    inputs = function.get("inputs", [])
    return {
        "inputs": [
            {"name": item.get("name"), "type_": item.get("type_")}
            for item in inputs
            if isinstance(item, dict)
        ],
        "outputs": function.get("outputs", []),
    }


def normalize_union(union: dict[str, Any]) -> list[dict[str, object]]:
    return [
        {"name": body["name"], "type_": body.get("type_", [])}
        for case in union.get("cases", [])
        if isinstance(case, dict)
        for body in case.values()
        if isinstance(body, dict) and isinstance(body.get("name"), str)
    ]


def normalize_error_enum(enum: dict[str, Any]) -> list[dict[str, object]]:
    return [
        {"name": case.get("name"), "value": case.get("value")}
        for case in enum.get("cases", [])
        if isinstance(case, dict)
    ]


def normalize_struct(struct_: dict[str, Any]) -> list[dict[str, object]]:
    fields = struct_.get("fields", [])
    return [
        {"name": field.get("name"), "type_": field.get("type_")}
        for field in fields
        if isinstance(field, dict)
    ]


def validate_interface(entries: object, policy_name: str) -> None:
    if not isinstance(entries, list) or not entries:
        fail("contract interface must be a nonempty array")
    functions: dict[str, dict[str, object]] = {}
    structs: dict[str, list[dict[str, object]]] = {}
    unions: dict[str, list[dict[str, object]]] = {}
    error_enums: dict[str, list[dict[str, object]]] = {}
    for entry in entries:
        if not isinstance(entry, dict):
            fail("contract interface entry must be an object")
        function = entry.get("function_v0")
        if isinstance(function, dict):
            name = function.get("name")
            if not isinstance(name, str) or not name:
                fail("contract interface function has no name")
            if name in functions:
                fail(f"duplicate function in contract interface: {name}")
            functions[name] = normalize_function(function)
        struct_ = entry.get("udt_struct_v0")
        if isinstance(struct_, dict):
            name = struct_.get("name")
            if isinstance(name, str):
                if name in structs:
                    fail(f"duplicate struct in contract interface: {name}")
                structs[name] = normalize_struct(struct_)
        union = entry.get("udt_union_v0")
        if isinstance(union, dict):
            name = union.get("name")
            if isinstance(name, str):
                if name in unions:
                    fail(f"duplicate union in contract interface: {name}")
                unions[name] = normalize_union(union)
        error_enum = entry.get("udt_error_enum_v0")
        if isinstance(error_enum, dict):
            name = error_enum.get("name")
            if isinstance(name, str):
                if name in error_enums:
                    fail(f"duplicate error enum in interface: {name}")
                error_enums[name] = normalize_error_enum(error_enum)
    policy = ABI_POLICIES.get(policy_name)
    if policy is None:
        fail(f"unknown ABI policy: {policy_name}")
    exported = policy.forbidden & functions.keys()
    if exported:
        fail(f"{policy_name}: forbidden exports: {sorted(exported)}")
    missing = policy.required - functions.keys()
    if missing:
        fail(f"{policy_name}: missing required exports: {sorted(missing)}")
    if (
        policy.exact_function_names is not None
        and set(functions) != policy.exact_function_names
    ):
        fail(f"{policy_name}: function set does not exactly match policy")
    for name, expected in policy.functions.items():
        if functions.get(name) != expected:
            fail(f"{policy_name}: ABI mismatch for function {name}")
    for name, expected in policy.structs.items():
        if structs.get(name) != expected:
            fail(f"{policy_name}: ABI mismatch for struct {name}")
    for name, expected_cases in policy.unions.items():
        if unions.get(name) != expected_cases:
            fail(f"{policy_name}: ABI mismatch for union {name}")
    for name, expected_cases in policy.error_enums.items():
        if error_enums.get(name) != expected_cases:
            fail(f"{policy_name}: ABI mismatch for error enum {name}")


def canonical_interface(entries: object, policy_name: str) -> bytes:
    validate_interface(entries, policy_name)
    assert isinstance(entries, list)
    return b"[" + b",".join(
        sorted(canonical_json(entry) for entry in entries)
    ) + b"]"


def interface_from_fd(fd: int, policy_name: str) -> tuple[list[object], str]:
    output = run_text(
        [
            "stellar",
            "--quiet",
            "contract",
            "info",
            "interface",
            "--wasm",
            f"{FD_DIRECTORY}/{fd}",
            "--output",
            "json",
        ],
        pass_fds=(fd,),
    )
    entries = parse_json(output)
    canonical = canonical_interface(entries, policy_name)
    assert isinstance(entries, list)
    return entries, sha256_bytes(canonical)


def validate_relative_path(relative: str) -> Path:
    if "\\" in relative:
        fail(f"noncanonical artifact path: {relative!r}")
    path = Path(relative)
    if (
        not relative
        or path.is_absolute()
        or relative != path.as_posix()
        or any(part in {"", ".", ".."} for part in path.parts)
    ):
        fail(f"noncanonical artifact path: {relative!r}")
    return path


@contextlib.contextmanager
def open_artifact(root: Path, relative: str) -> Iterator[tuple[int, os.stat_result]]:
    path = validate_relative_path(relative)
    directory_flags = (
        os.O_RDONLY
        | os.O_DIRECTORY
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_CLOEXEC", 0)
    )
    parent = os.open(root.resolve(strict=True), directory_flags)
    try:
        for part in path.parts[:-1]:
            nested = os.open(part, directory_flags, dir_fd=parent)
            os.close(parent)
            parent = nested
        metadata = os.stat(path.name, dir_fd=parent, follow_symlinks=False)
        if not stat.S_ISREG(metadata.st_mode):
            os.close(parent)
            fail(f"artifact is not a regular nonsymlink file: {relative}")
        flags = (
            os.O_RDONLY
            | getattr(os, "O_NOFOLLOW", 0)
            | getattr(os, "O_CLOEXEC", 0)
            | getattr(os, "O_NONBLOCK", 0)
        )
        fd = os.open(path.name, flags, dir_fd=parent)
    except OSError:
        os.close(parent)
        fail(
            f"artifact or an ancestor is not a regular nonsymlink: {relative}"
            " (a relocated build directory must be a real path, not a symlink)"
        )
    os.close(parent)
    try:
        opened = os.fstat(fd)
        if not stat.S_ISREG(opened.st_mode) or opened.st_nlink != 1:
            fail(f"artifact must be a single-link regular file: {relative}")
        yield fd, opened
    finally:
        os.close(fd)


def sha256_fd(fd: int) -> str:
    digest = hashlib.sha256()
    os.lseek(fd, 0, os.SEEK_SET)
    while chunk := os.read(fd, 65_536):
        digest.update(chunk)
    os.lseek(fd, 0, os.SEEK_SET)
    return digest.hexdigest()


@dataclass(frozen=True)
class ArtifactMeasurement:
    size: int
    wasm_sha256: str
    contract_spec_sha256: str


def inspect_artifact_fd(fd: int, artifact: Artifact) -> ArtifactMeasurement:
    metadata = os.fstat(fd)
    if (
        not stat.S_ISREG(metadata.st_mode)
        or metadata.st_nlink != 1
        or metadata.st_size <= 0
    ):
        fail(f"{artifact.slug}: artifact is not a positive single-link regular file")
    if metadata.st_size > artifact.max_optimized_size:
        fail(
            f"{artifact.slug}: {metadata.st_size} bytes exceeds "
            f"{artifact.max_optimized_size} byte budget"
        )
    wasm_digest = sha256_fd(fd)
    _, spec_digest = interface_from_fd(fd, artifact.spec_policy)
    if (
        os.fstat(fd).st_size != metadata.st_size
        or sha256_fd(fd) != wasm_digest
    ):
        fail(f"{artifact.slug}: artifact changed during inspection")
    return ArtifactMeasurement(
        size=metadata.st_size,
        wasm_sha256=wasm_digest,
        contract_spec_sha256=spec_digest,
    )

def read_single_link_file(path: Path, label: str) -> bytes:
    if path.is_symlink():
        fail(f"{label} must not be a symlink")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
    descriptor = os.open(path, flags)
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
            fail(f"{label} must be a single-link regular file")
        chunks: list[bytes] = []
        while chunk := os.read(descriptor, 65_536):
            chunks.append(chunk)
        return b"".join(chunks)
    finally:
        os.close(descriptor)



def git_head(root: Path = ROOT) -> str:
    head = run_text(["git", "rev-parse", "HEAD"], cwd=root)
    if not re.fullmatch(r"[0-9a-f]{40}", head):
        fail("Git HEAD is not a lowercase 40-hex commit")
    return head


def require_tracked_clean(root: Path = ROOT) -> None:
    """Refuse to release a commit that is not what the operator is looking at.

    A release builds from an export of the commit, so uncommitted work cannot
    reach the bytes; this only stops someone from shipping a commit whose
    changes they have not saved. Untracked files are therefore none of its
    business.
    """
    status = run_text(["git", "status", "--porcelain", "-uno"], cwd=root)
    if status:
        offending = sorted(
            path
            for path in (line[2:].strip() for line in status.splitlines())
            if path
        )
        fail(
            "tracked source must be clean; offending paths: "
            + ", ".join(offending[:10])
        )


def fsync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def atomic_write(
    path: Path,
    content: bytes,
    mode: int = 0o644,
    *,
    dir_mode: int = 0o777,
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=dir_mode)
    descriptor, raw = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(raw)
    try:
        os.fchmod(descriptor, mode)
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        fsync_directory(path.parent)
    finally:
        temporary.unlink(missing_ok=True)


def invalidate_release_evidence() -> None:
    MANIFEST_PATH.unlink(missing_ok=True)
    EVIDENCE_PATH.unlink(missing_ok=True)
def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group()
    group.add_argument(
        "--wasm-path",
        metavar="SLUG",
        choices=tuple(artifact.slug for artifact in ARTIFACTS),
        help="print the optimized Wasm path for one artifact",
    )
    group.add_argument(
        "--sizes",
        action="store_true",
        help="print slug, size (or 'missing'), and path for every artifact",
    )
    args = parser.parse_args()
    try:
        if args.wasm_path:
            print(WASM_DIR / catalog_artifact(args.wasm_path).optimized_wasm)
        elif args.sizes:
            print(sizes_tsv())
        else:
            print(catalog_tsv())
    except (OSError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
