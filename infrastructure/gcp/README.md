# GCP Infrastructure (OpenTofu)

This stack provisions the GCP foundation for a container-based CI/CD flow:

- Required project APIs
- Artifact Registry Docker repository
- GitHub Actions service account
- Workload Identity Federation (GitHub OIDC -> GCP SA impersonation)

This is enough to support the next step of pushing images from GitHub Actions without static JSON keys.

## Prerequisites

- OpenTofu `>= 1.8`
- GCP project with billing enabled
- A principal with permission to manage IAM, Service Usage, and Artifact Registry

## Files

- `versions.tf`: provider and version constraints
- `providers.tf`: Google provider config
- `variables.tf`: input variables
- `main.tf`: resources
- `outputs.tf`: values used by CI
- `terraform.tfvars.example`: sample inputs

## Usage

```bash
cd infrastructure/gcp
cp terraform.tfvars.example terraform.tfvars
# edit terraform.tfvars

tofu init
tofu plan
tofu apply
```

## Key outputs

After apply, capture:

- `github_workload_identity_provider`
- `github_actions_service_account_email`
- `artifact_registry_host`
- `artifact_registry_image_base`

Use those in GitHub Actions secrets/variables for build/push jobs.

## GitHub repository configuration

Set these in your GitHub repository after `tofu apply`:

- `Secrets`
  - `GCP_WORKLOAD_IDENTITY_PROVIDER` = `github_workload_identity_provider`
  - `GCP_SERVICE_ACCOUNT_EMAIL` = `github_actions_service_account_email`
- `Variables`
  - `GCP_ARTIFACT_REGISTRY_HOST` = `artifact_registry_host`
  - `GCP_ARTIFACT_REGISTRY_IMAGE_BASE` = `artifact_registry_image_base`

The workflow `.github/workflows/build-service-image.yml` uses these values to authenticate with OIDC and push images to Artifact Registry.

## Example GitHub Actions auth step

```yaml
permissions:
  contents: read
  id-token: write

steps:
  - uses: actions/checkout@v4
  - uses: google-github-actions/auth@v2
    with:
      workload_identity_provider: ${{ secrets.GCP_WORKLOAD_IDENTITY_PROVIDER }}
      service_account: ${{ secrets.GCP_SERVICE_ACCOUNT_EMAIL }}
  - uses: google-github-actions/setup-gcloud@v2
  - run: gcloud auth configure-docker ${{ vars.GCP_ARTIFACT_REGISTRY_HOST }} --quiet
```

## Notes

- OIDC trust is restricted to `github_repository`.
- Branch/tag-level restrictions can be added later using stricter provider conditions or IAM conditions once your deployment flow is finalized.
