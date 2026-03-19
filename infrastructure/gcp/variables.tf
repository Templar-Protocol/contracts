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

variable "enable_runtime" {
  description = "Whether to provision GCE runtime infrastructure for services."
  type        = bool
  default     = true
}

variable "runtime_service_account_id" {
  description = "Service account ID attached to runtime compute instances."
  type        = string
  default     = "templar-runtime"
}

variable "runtime_service_account_display_name" {
  description = "Display name for the runtime service account."
  type        = string
  default     = "Templar Runtime"
}

variable "runtime_service_account_project_roles" {
  description = "Project IAM roles for the runtime service account."
  type        = list(string)
  default = [
    "roles/artifactregistry.reader",
    "roles/logging.logWriter",
    "roles/monitoring.metricWriter"
  ]
}

variable "runtime_network_name" {
  description = "Name of the VPC network used by runtime instances."
  type        = string
  default     = "templar-runtime-vpc"
}

variable "runtime_subnet_name" {
  description = "Name of the runtime subnet."
  type        = string
  default     = "templar-runtime-subnet"
}

variable "runtime_subnet_cidr" {
  description = "CIDR range for the runtime subnet."
  type        = string
  default     = "10.42.0.0/24"
}

variable "runtime_source_image" {
  description = "Boot image for runtime instances."
  type        = string
  default     = "projects/debian-cloud/global/images/family/debian-12"
}

variable "admin_source_ranges" {
  description = "Source CIDRs allowed to SSH into runtime instances."
  type        = list(string)
  default     = []
}

variable "relayer_enabled" {
  description = "Whether to provision relayer runtime infrastructure."
  type        = bool
  default     = true
}

variable "relayer_image" {
  description = "Relayer container image. If null, uses the default Artifact Registry templar image."
  type        = string
  default     = null
  nullable    = true
}

variable "relayer_container_name" {
  description = "Docker container name for relayer instances."
  type        = string
  default     = "templar-relayer"
}

variable "relayer_machine_type" {
  description = "Machine type for relayer instances."
  type        = string
  default     = "e2-standard-4"
}

variable "relayer_disk_size_gb" {
  description = "Boot disk size for relayer instances."
  type        = number
  default     = 40
}

variable "relayer_port" {
  description = "Port exposed by the relayer service."
  type        = number
  default     = 3000

  validation {
    condition     = var.relayer_port > 0 && var.relayer_port < 65536
    error_message = "relayer_port must be between 1 and 65535."
  }
}

variable "relayer_instance_count" {
  description = "Desired relayer instance count (used when autoscaling is disabled)."
  type        = number
  default     = 2

  validation {
    condition     = var.relayer_instance_count >= 1
    error_message = "relayer_instance_count must be at least 1."
  }
}

variable "relayer_zones" {
  description = "Zones used for relayer regional distribution. Empty means region-b and region-c."
  type        = list(string)
  default     = []
}

variable "relayer_allowed_source_ranges" {
  description = "Source CIDRs allowed to reach relayer TCP port."
  type        = list(string)
  default     = []
}

variable "relayer_autoscaling_enabled" {
  description = "Whether to enable autoscaling for relayer MIG."
  type        = bool
  default     = true
}

variable "relayer_min_replicas" {
  description = "Minimum relayer replicas for autoscaling."
  type        = number
  default     = 2

  validation {
    condition     = var.relayer_min_replicas >= 1
    error_message = "relayer_min_replicas must be at least 1."
  }
}

variable "relayer_max_replicas" {
  description = "Maximum relayer replicas for autoscaling."
  type        = number
  default     = 6

  validation {
    condition     = var.relayer_max_replicas >= var.relayer_min_replicas
    error_message = "relayer_max_replicas must be greater than or equal to relayer_min_replicas."
  }
}

variable "relayer_cpu_target" {
  description = "Target CPU utilization (0-1) for relayer autoscaling."
  type        = number
  default     = 0.65

  validation {
    condition     = var.relayer_cpu_target > 0 && var.relayer_cpu_target < 1
    error_message = "relayer_cpu_target must be between 0 and 1."
  }
}

variable "relayer_env" {
  description = "Non-secret environment variables injected into relayer container."
  type        = map(string)
  default     = {}
  sensitive   = true
}

variable "relayer_secret_env" {
  description = "Secret Manager bindings for relayer environment variables (ENV_VAR => SECRET_ID)."
  type        = map(string)
  default     = {}

  validation {
    condition     = alltrue([for secret_id in values(var.relayer_secret_env) : can(regex("^[A-Za-z0-9_-]+$", secret_id))])
    error_message = "relayer_secret_env values must be Secret Manager secret IDs (letters, numbers, underscores, hyphens)."
  }
}

variable "market_monitor_enabled" {
  description = "Whether to provision market-monitor runtime infrastructure."
  type        = bool
  default     = true
}

variable "market_monitor_image" {
  description = "Market-monitor container image. If null, uses the default Artifact Registry templar image."
  type        = string
  default     = null
  nullable    = true
}

variable "market_monitor_container_name" {
  description = "Docker container name for market-monitor instance."
  type        = string
  default     = "templar-market-monitor"
}

variable "market_monitor_machine_type" {
  description = "Machine type for market-monitor instance."
  type        = string
  default     = "e2-small"
}

variable "market_monitor_disk_size_gb" {
  description = "Boot disk size for market-monitor instance."
  type        = number
  default     = 20
}

variable "market_monitor_zone" {
  description = "Zone for the market-monitor instance. If null, uses first relayer zone."
  type        = string
  default     = null
  nullable    = true
}

variable "market_monitor_env" {
  description = "Non-secret environment variables injected into market-monitor container."
  type        = map(string)
  default     = {}
  sensitive   = true
}

variable "market_monitor_secret_env" {
  description = "Secret Manager bindings for market-monitor environment variables (ENV_VAR => SECRET_ID)."
  type        = map(string)
  default     = {}

  validation {
    condition     = alltrue([for secret_id in values(var.market_monitor_secret_env) : can(regex("^[A-Za-z0-9_-]+$", secret_id))])
    error_message = "market_monitor_secret_env values must be Secret Manager secret IDs (letters, numbers, underscores, hyphens)."
  }
}

variable "accumulator_enabled" {
  description = "Whether to provision accumulator runtime infrastructure."
  type        = bool
  default     = true
}

variable "accumulator_image" {
  description = "Accumulator container image. If null, uses the default Artifact Registry templar image."
  type        = string
  default     = null
  nullable    = true
}

variable "accumulator_container_name" {
  description = "Docker container name for accumulator instance."
  type        = string
  default     = "templar-accumulator"
}

variable "accumulator_machine_type" {
  description = "Machine type for accumulator instance."
  type        = string
  default     = "e2-micro"
}

variable "accumulator_disk_size_gb" {
  description = "Boot disk size for accumulator instance."
  type        = number
  default     = 20
}

variable "accumulator_zone" {
  description = "Zone for the accumulator instance. If null, uses first relayer zone."
  type        = string
  default     = null
  nullable    = true
}

variable "accumulator_env" {
  description = "Non-secret environment variables injected into accumulator container."
  type        = map(string)
  default     = {}
  sensitive   = true
}

variable "accumulator_secret_env" {
  description = "Secret Manager bindings for accumulator environment variables (ENV_VAR => SECRET_ID)."
  type        = map(string)
  default     = {}

  validation {
    condition     = alltrue([for secret_id in values(var.accumulator_secret_env) : can(regex("^[A-Za-z0-9_-]+$", secret_id))])
    error_message = "accumulator_secret_env values must be Secret Manager secret IDs (letters, numbers, underscores, hyphens)."
  }
}

variable "funding_bridge_enabled" {
  description = "Whether to provision funding-bridge runtime infrastructure."
  type        = bool
  default     = true
}

variable "funding_bridge_image" {
  description = "Funding-bridge container image. If null, uses the default Artifact Registry templar image."
  type        = string
  default     = null
  nullable    = true
}

variable "funding_bridge_container_name" {
  description = "Docker container name for funding-bridge instance."
  type        = string
  default     = "templar-funding-bridge"
}

variable "funding_bridge_machine_type" {
  description = "Machine type for funding-bridge instance."
  type        = string
  default     = "e2-small"
}

variable "funding_bridge_disk_size_gb" {
  description = "Boot disk size for funding-bridge instance."
  type        = number
  default     = 20
}

variable "funding_bridge_zone" {
  description = "Zone for the funding-bridge instance. If null, uses first relayer zone."
  type        = string
  default     = null
  nullable    = true
}

variable "funding_bridge_port" {
  description = "Port exposed by the funding-bridge service."
  type        = number
  default     = 3000

  validation {
    condition     = var.funding_bridge_port > 0 && var.funding_bridge_port < 65536
    error_message = "funding_bridge_port must be between 1 and 65535."
  }
}

variable "funding_bridge_allowed_source_ranges" {
  description = "Source CIDRs allowed to reach funding-bridge TCP port."
  type        = list(string)
  default     = []
}

variable "funding_bridge_env" {
  description = "Non-secret environment variables injected into funding-bridge container."
  type        = map(string)
  default     = {}
  sensitive   = true
}

variable "funding_bridge_secret_env" {
  description = "Secret Manager bindings for funding-bridge environment variables (ENV_VAR => SECRET_ID)."
  type        = map(string)
  default     = {}

  validation {
    condition     = alltrue([for secret_id in values(var.funding_bridge_secret_env) : can(regex("^[A-Za-z0-9_-]+$", secret_id))])
    error_message = "funding_bridge_secret_env values must be Secret Manager secret IDs (letters, numbers, underscores, hyphens)."
  }
}
