# GCP Infrastructure (OpenTofu)

This stack provisions both CI/CD integration and runtime infrastructure:

- Required project APIs
- Artifact Registry Docker repository
- GitHub Actions OIDC federation (no static JSON key)
- Runtime VPC + subnet
- Runtime service account and IAM
- Relayer runtime as a regional managed instance group (HA + autoscaling)
- Market-monitor runtime as a single Compute Engine VM
- Funding-bridge runtime as a single Compute Engine VM
- Accumulator runtime as a lightweight single Compute Engine VM

`liquidator` is intentionally not included in this runtime stack.

## Prerequisites

- OpenTofu `>= 1.8`
- GCP project with billing enabled
- A principal with permission to manage IAM, Service Usage, and Compute resources

## Files

- `versions.tf`: provider and version constraints
- `providers.tf`: Google provider config
- `variables.tf`: input variables
- `main.tf`: CI/CD foundation resources
- `runtime.tf`: runtime compute resources
- `outputs.tf`: values used by CI and operations
- `terraform.tfvars.example`: sample inputs

## Usage

```bash
cd infrastructure/gcp
cp terraform.tfvars.example terraform.tfvars
# edit terraform.tfvars

tofu init
tofu workspace select -or-create dev
tofu plan
tofu apply
```

State backend is fixed in `backend.tf`:

- bucket: `templar-tfstate`
- prefix: `templar/gcp`

Use workspaces to separate environments (`dev`, `main`):

- `dev` workspace state
- `main` workspace state

For local validation without backend:

```bash
tofu init -backend=false
tofu validate
```

## Runtime architecture

- `relayer`
  - `google_compute_region_instance_group_manager`
  - default 2 replicas across zones (`<region>-b`, `<region>-c`)
  - health check + autohealing
  - optional autoscaling by CPU
  - high-performance default machine type: `e2-standard-4`
- `accumulator`
  - `google_compute_instance` (single node)
  - lightweight default machine type: `e2-micro`
  - default schedule-oriented env (`INTERVAL=43200`, `STATIC_INTERVAL=86400`)
- `market-monitor`
  - `google_compute_instance` (single node)
  - default machine type: `e2-small`
- `funding-bridge`
  - `google_compute_instance` (single node)
  - default machine type: `e2-small`
  - ingress firewall on `funding_bridge_port` (default `3000`)

All runtime services run the pushed Artifact Registry image and execute service-specific binaries:

- relayer: `/app/bin/templar-relayer`
- market-monitor: `/app/bin/templar-market-monitor`
- funding-bridge: `/app/bin/templar-funding-bridge`
- accumulator: `/app/bin/templar-accumulator`

## Required runtime env

Set service env maps in `terraform.tfvars`:

- `relayer_env`, `market_monitor_env`, `funding_bridge_env`, `accumulator_env`: non-secret environment variables only
- `relayer_secret_env`, `market_monitor_secret_env`, `funding_bridge_secret_env`, `accumulator_secret_env`: secret bindings in the form `ENV_VAR => SECRET_ID`

Secrets are fetched from Secret Manager at instance boot and written to the local runtime env file only on the VM.

## GitHub repository configuration

Set these in your GitHub repository after `tofu apply`:

- `Secrets`
  - `GCP_WORKLOAD_IDENTITY_PROVIDER` = `github_workload_identity_provider`
  - `GCP_SERVICE_ACCOUNT_EMAIL` = `github_actions_service_account_email`
- `Variables`
  - `GCP_PROJECT_ID` = your GCP project ID
  - `GCP_ARTIFACT_REGISTRY_HOST` = `artifact_registry_host`
  - `GCP_ARTIFACT_REGISTRY_IMAGE_BASE` = `artifact_registry_image_base`

Workflows:

- `.github/workflows/build-service-image.yml`: builds and pushes container images
- `.github/workflows/infrastructure-gcp.yml`: runs `tofu plan` on PRs and `tofu apply` on pushes to `main`/`dev`

## Key outputs

CI/CD outputs:

- `github_workload_identity_provider`
- `github_actions_service_account_email`
- `artifact_registry_host`
- `artifact_registry_image_base`

Runtime outputs:

- `runtime_service_account_email`
- `runtime_network`
- `relayer_instance_group`
- `relayer_region`
- `relayer_service_port`
- `market_monitor_instance_name`
- `market_monitor_external_ip`
- `funding_bridge_instance_name`
- `funding_bridge_external_ip`
- `funding_bridge_service_port`
- `accumulator_instance_name`
- `accumulator_external_ip`

## Notes

- `relayer_allowed_source_ranges` defaults to `0.0.0.0/0`; tighten this for production.
- `funding_bridge_allowed_source_ranges` defaults to `0.0.0.0/0`; tighten this for production.
- Do not put secret values in `*_env` maps; only use `*_secret_env` mappings to Secret Manager secret IDs.
- This stack does not yet provision a public load balancer for relayer; add one in a follow-up if you need a single stable endpoint.
