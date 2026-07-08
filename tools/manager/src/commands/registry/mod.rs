mod add_version;
mod clear_deployments;
mod deploy;
mod get_deployment;
mod list_deployments;
mod list_deployments_by_kind;
mod list_versions;
mod remove;
mod remove_version;

pub use add_version::AddVersion;
pub use clear_deployments::ClearDeployments;
pub use deploy::Deploy;
pub use get_deployment::GetDeployment;
pub use list_deployments::ListDeployments;
pub use list_deployments_by_kind::ListDeploymentsByKind;
pub use list_versions::ListVersions;
pub use remove::Remove;
pub use remove_version::RemoveVersion;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum RegistryNs {
    ListVersions(ListVersions),
    ListDeployments(ListDeployments),
    ListDeploymentsByKind(ListDeploymentsByKind),
    GetDeployment(GetDeployment),
    AddVersion(AddVersion),
    Deploy(Deploy),
    /// Remove a single version, or every version with `--all`.
    RemoveVersion(RemoveVersion),
    /// Remove every version from the registry, then delete the (signer) account.
    Remove(Remove),
    /// Remove every market deployed from the registry (signing as each with the
    /// shared `--secret-key`).
    ClearDeployments(ClearDeployments),
}
