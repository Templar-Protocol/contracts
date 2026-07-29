//! Manifest contract lookup, adapter indexing, and status projections.

use std::collections::BTreeMap;

use anyhow::Context;

use crate::{cli::AdapterArgs, manifest::Manifest, types::AddressStr};

use super::output::{BlendAdapterStatus, CustodialAdapterStatus, StatusResponse};

pub(super) fn contract_id<'a>(manifest: &'a Manifest, key: &str) -> Option<&'a str> {
    manifest
        .contracts
        .get(key)
        .map(|record| record.contract_id.as_str())
}

pub(super) fn required_contract<'a>(manifest: &'a Manifest, key: &str) -> anyhow::Result<&'a str> {
    contract_id(manifest, key).with_context(|| format!("missing {key} contract id in manifest"))
}

pub(super) fn blend_adapter_key(index: usize) -> String {
    format!("blend_adapter_{index}")
}

pub(super) fn next_blend_adapter_key(manifest: &Manifest) -> String {
    blend_adapter_key(next_blend_adapter_index(manifest))
}

pub(super) fn next_blend_adapter_index(manifest: &Manifest) -> usize {
    let highest_index = manifest
        .contracts
        .keys()
        .filter_map(|key| {
            if key == "blend_adapter" {
                Some(0)
            } else {
                blend_adapter_index(key)
            }
        })
        .max();
    highest_index.map_or(0, |index| index + 1)
}

pub(super) fn blend_adapter_by_pool<'a>(
    manifest: &'a Manifest,
    pool: &AddressStr,
) -> Option<&'a str> {
    manifest
        .contracts
        .iter()
        .find(|(key, record)| {
            is_blend_adapter_key(key)
                && record
                    .constructor_args
                    .get("pool")
                    .is_some_and(|value| value == pool.as_str())
        })
        .map(|(_, record)| record.contract_id.as_str())
}

pub(super) fn custodial_adapter_key(index: usize) -> String {
    format!("custodial_adapter_{index}")
}

pub(super) fn next_custodial_adapter_key(manifest: &Manifest) -> String {
    custodial_adapter_key(next_custodial_adapter_index(manifest))
}

pub(super) fn next_custodial_adapter_index(manifest: &Manifest) -> usize {
    let highest_index = manifest
        .contracts
        .keys()
        .filter_map(|key| custodial_adapter_index(key))
        .max();
    highest_index.map_or(0, |index| index + 1)
}

pub(super) fn custodial_adapter_by_custodian<'a>(
    manifest: &'a Manifest,
    custodian: &AddressStr,
) -> Option<&'a str> {
    manifest
        .contracts
        .iter()
        .find(|(key, record)| {
            is_custodial_adapter_key(key)
                && record
                    .constructor_args
                    .get("custodian")
                    .is_some_and(|value| value == custodian.as_str())
        })
        .map(|(_, record)| record.contract_id.as_str())
}

pub(super) fn selected_blend_adapter<'a>(
    manifest: &'a Manifest,
    args: &AdapterArgs,
) -> anyhow::Result<&'a str> {
    if let Some(key) = &args.adapter_key {
        return required_contract(manifest, key);
    }
    if let Some(pool) = &args.adapter_pool {
        return blend_adapter_by_pool(manifest, pool)
            .with_context(|| format!("missing Blend adapter for pool {pool}"));
    }

    let key = blend_adapter_key(args.adapter_index);
    contract_id(manifest, &key)
        .or_else(|| {
            if args.adapter_index == 0 {
                contract_id(manifest, "blend_adapter")
            } else {
                None
            }
        })
        .with_context(|| format!("missing {key} contract id in manifest"))
}

pub(super) fn is_blend_adapter_key(key: &str) -> bool {
    key == "blend_adapter" || blend_adapter_index(key).is_some()
}

pub(super) fn blend_adapter_index(key: &str) -> Option<usize> {
    key.strip_prefix("blend_adapter_")?.parse().ok()
}

pub(super) fn is_custodial_adapter_key(key: &str) -> bool {
    custodial_adapter_index(key).is_some()
}

pub(super) fn custodial_adapter_index(key: &str) -> Option<usize> {
    key.strip_prefix("custodial_adapter_")?.parse().ok()
}

pub(super) fn args<const N: usize>(items: [(&str, &str); N]) -> Vec<String> {
    items
        .into_iter()
        .flat_map(|(key, value)| [key.to_string(), value.to_string()])
        .collect()
}

pub(super) fn map_args<const N: usize>(items: [(&str, &str); N]) -> BTreeMap<String, String> {
    items
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

pub(super) fn parse_proposal_id(stdout: &str) -> anyhow::Result<u64> {
    let proposal_output = stdout
        .lines()
        .take_while(|line| {
            !line
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("tx hash:")
        })
        .collect::<Vec<_>>()
        .join("\n");
    proposal_output
        .split(|c: char| !c.is_ascii_digit())
        .rev()
        .find(|part| !part.is_empty())
        .context("no proposal id found in governance output")?
        .parse()
        .context("parse proposal id")
}

pub(super) fn status_response(manifest: &Manifest) -> StatusResponse {
    StatusResponse {
        network: manifest.network.clone(),
        vault: contract_id(manifest, "vault").map(ToString::to_string),
        share_token: contract_id(manifest, "share_token").map(ToString::to_string),
        governance: contract_id(manifest, "governance").map(ToString::to_string),
        asset_token: contract_id(manifest, "asset_token").map(ToString::to_string),
        proxy_4626: contract_id(manifest, "proxy_4626").map(ToString::to_string),
        curator_proxy: contract_id(manifest, "curator_proxy").map(ToString::to_string),
        blend_adapters: blend_adapter_statuses(manifest),
        custodial_adapters: custodial_adapter_statuses(manifest),
    }
}

pub(super) fn blend_adapter_statuses(manifest: &Manifest) -> Vec<BlendAdapterStatus> {
    let mut adapters = manifest
        .contracts
        .iter()
        .filter_map(|(key, record)| {
            let index = blend_adapter_index(key)?;
            Some((
                index,
                BlendAdapterStatus {
                    key: key.clone(),
                    contract_id: record.contract_id.clone(),
                    pool: record.constructor_args.get("pool").cloned(),
                },
            ))
        })
        .collect::<Vec<_>>();
    adapters.sort_by_key(|(index, _)| *index);
    let mut adapters = adapters
        .into_iter()
        .map(|(_, status)| status)
        .collect::<Vec<_>>();
    if adapters.is_empty() {
        if let Some(record) = manifest.contracts.get("blend_adapter") {
            adapters.push(BlendAdapterStatus {
                key: "blend_adapter".to_string(),
                contract_id: record.contract_id.clone(),
                pool: record.constructor_args.get("pool").cloned(),
            });
        }
    }
    adapters
}

pub(super) fn custodial_adapter_statuses(manifest: &Manifest) -> Vec<CustodialAdapterStatus> {
    let mut adapters = manifest
        .contracts
        .iter()
        .filter_map(|(key, record)| {
            let index = custodial_adapter_index(key)?;
            Some((
                index,
                CustodialAdapterStatus {
                    key: key.clone(),
                    contract_id: record.contract_id.clone(),
                    custodian: record.constructor_args.get("custodian").cloned(),
                    asset: record.constructor_args.get("asset").cloned(),
                },
            ))
        })
        .collect::<Vec<_>>();
    adapters.sort_by_key(|(index, _)| *index);
    adapters
        .into_iter()
        .map(|(_, status)| status)
        .collect::<Vec<_>>()
}

pub(super) fn export_env(manifest: &Manifest) -> Vec<(String, String)> {
    let mut values = vec![("SOROBAN_NETWORK".to_string(), manifest.network.clone())];
    for (env, key) in [
        ("SOROBAN_CONTRACT_ID", "vault"),
        ("SOROBAN_SHARE_TOKEN", "share_token"),
        ("SOROBAN_GOVERNANCE", "governance"),
        ("SOROBAN_ASSET_TOKEN", "asset_token"),
        ("SOROBAN_4626_PROXY", "proxy_4626"),
        ("SOROBAN_CURATOR_PROXY", "curator_proxy"),
    ] {
        if let Some(value) = contract_id(manifest, key) {
            values.push((env.to_string(), value.to_string()));
        }
    }
    for (index, adapter) in blend_adapter_statuses(manifest).into_iter().enumerate() {
        if index == 0 {
            values.push(("BLEND_ADAPTER_ID".to_string(), adapter.contract_id.clone()));
        }
        values.push((
            format!("BLEND_ADAPTER_{index}_ID"),
            adapter.contract_id.clone(),
        ));
        if let Some(pool) = adapter.pool {
            values.push((format!("BLEND_POOL_{index}_ID"), pool));
        }
    }
    for (index, adapter) in custodial_adapter_statuses(manifest).into_iter().enumerate() {
        if index == 0 {
            values.push((
                "CUSTODIAL_ADAPTER_ID".to_string(),
                adapter.contract_id.clone(),
            ));
        }
        values.push((
            format!("CUSTODIAL_ADAPTER_{index}_ID"),
            adapter.contract_id.clone(),
        ));
        if let Some(custodian) = adapter.custodian {
            values.push((format!("CUSTODIAL_{index}_ADDRESS"), custodian));
        }
        if let Some(asset) = adapter.asset {
            values.push((format!("CUSTODIAL_{index}_ASSET"), asset));
        }
    }
    values
}
