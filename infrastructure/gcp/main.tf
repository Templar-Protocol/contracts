data "google_project" "current" {
  project_id = var.project_id
}

locals {
  artifact_registry_location = coalesce(var.artifact_registry_location, var.region)

  required_project_services = toset(concat([
    "artifactregistry.googleapis.com",
    "compute.googleapis.com",
    "iam.googleapis.com",
    "iamcredentials.googleapis.com",
    "secretmanager.googleapis.com",
    "sts.googleapis.com",
    "serviceusage.googleapis.com"
  ], var.additional_project_services))
}

resource "google_project_service" "required" {
  for_each = local.required_project_services

  project                    = var.project_id
  service                    = each.value
  disable_dependent_services = false
  disable_on_destroy         = false
}

resource "google_artifact_registry_repository" "services" {
  project       = var.project_id
  location      = local.artifact_registry_location
  repository_id = var.artifact_registry_repository_id
  description   = var.artifact_registry_description
  format        = "DOCKER"

  depends_on = [google_project_service.required]
}

resource "google_service_account" "github_actions" {
  project      = var.project_id
  account_id   = var.github_actions_service_account_id
  display_name = var.github_actions_service_account_display_name
}

resource "google_project_iam_member" "github_actions_roles" {
  for_each = toset(var.github_actions_service_account_project_roles)

  project = var.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.github_actions.email}"
}

resource "google_iam_workload_identity_pool" "github" {
  count = var.enable_github_wif ? 1 : 0

  project                   = var.project_id
  workload_identity_pool_id = var.workload_identity_pool_id
  display_name              = "GitHub Actions Pool"
  description               = "OIDC federation for GitHub Actions"

  depends_on = [google_project_service.required]
}

resource "google_iam_workload_identity_pool_provider" "github" {
  count = var.enable_github_wif ? 1 : 0

  project                            = var.project_id
  workload_identity_pool_id          = google_iam_workload_identity_pool.github[0].workload_identity_pool_id
  workload_identity_pool_provider_id = var.workload_identity_provider_id
  display_name                       = "GitHub Actions OIDC"
  description                        = "Trust token.actions.githubusercontent.com for Templar CI/CD"

  attribute_mapping = {
    "google.subject"       = "assertion.sub"
    "attribute.actor"      = "assertion.actor"
    "attribute.aud"        = "assertion.aud"
    "attribute.ref"        = "assertion.ref"
    "attribute.repository" = "assertion.repository"
  }

  attribute_condition = "assertion.repository == '${var.github_repository}'"

  oidc {
    issuer_uri = "https://token.actions.githubusercontent.com"
  }
}

resource "google_service_account_iam_member" "github_actions_wif_user" {
  count = var.enable_github_wif ? 1 : 0

  service_account_id = google_service_account.github_actions.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "principalSet://iam.googleapis.com/${google_iam_workload_identity_pool.github[0].name}/attribute.repository/${var.github_repository}"
}
