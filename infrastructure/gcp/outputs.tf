output "project_number" {
  description = "Numeric project identifier."
  value       = data.google_project.current.number
}

output "artifact_registry_host" {
  description = "Registry host to use in docker login/tag commands."
  value       = "${local.artifact_registry_location}-docker.pkg.dev"
}

output "artifact_registry_repository" {
  description = "Artifact Registry repository resource name."
  value       = google_artifact_registry_repository.services.name
}

output "artifact_registry_image_base" {
  description = "Base image path for service images."
  value       = "${local.artifact_registry_location}-docker.pkg.dev/${var.project_id}/${google_artifact_registry_repository.services.repository_id}"
}

output "github_actions_service_account_email" {
  description = "Service account email for GitHub Actions auth."
  value       = google_service_account.github_actions.email
}

output "github_workload_identity_provider" {
  description = "Full Workload Identity Provider resource name for google-github-actions/auth."
  value       = try(google_iam_workload_identity_pool_provider.github[0].name, null)
}
