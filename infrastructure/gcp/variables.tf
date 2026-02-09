variable "project_id" {
  description = "GCP project ID where infrastructure will be created."
  type        = string
}

variable "region" {
  description = "Default GCP region for regional resources."
  type        = string
  default     = "europe-west3"
}

variable "artifact_registry_location" {
  description = "Location for Artifact Registry. If null, uses region."
  type        = string
  default     = null
  nullable    = true
}

variable "artifact_registry_repository_id" {
  description = "Artifact Registry repository ID for Docker images."
  type        = string
  default     = "templar-services"
}

variable "artifact_registry_description" {
  description = "Description for the Artifact Registry repository."
  type        = string
  default     = "Templar service container images"
}

variable "github_repository" {
  description = "GitHub repository allowed to impersonate the CI service account (format: owner/repo)."
  type        = string

  validation {
    condition     = can(regex("^[^/]+/[^/]+$", var.github_repository))
    error_message = "github_repository must be in the format owner/repo."
  }
}

variable "enable_github_wif" {
  description = "Whether to create Workload Identity Federation resources for GitHub Actions."
  type        = bool
  default     = true
}

variable "github_actions_service_account_id" {
  description = "Service account ID used by GitHub Actions (max 30 chars)."
  type        = string
  default     = "templar-github-ci"
}

variable "github_actions_service_account_display_name" {
  description = "Display name of the GitHub Actions service account."
  type        = string
  default     = "Templar GitHub CI"
}

variable "github_actions_service_account_project_roles" {
  description = "Project IAM roles granted to the GitHub Actions service account."
  type        = list(string)
  default = [
    "roles/artifactregistry.reader",
    "roles/artifactregistry.writer"
  ]
}

variable "workload_identity_pool_id" {
  description = "Workload Identity Pool ID for GitHub OIDC."
  type        = string
  default     = "templar-gh-pool"
}

variable "workload_identity_provider_id" {
  description = "Workload Identity Provider ID for GitHub OIDC."
  type        = string
  default     = "templar-gh-provider"
}

variable "additional_project_services" {
  description = "Additional Google APIs to enable in the project."
  type        = list(string)
  default     = []
}
