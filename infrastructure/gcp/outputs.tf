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

output "runtime_service_account_email" {
  description = "Service account email used by runtime instances."
  value       = try(google_service_account.runtime[0].email, null)
}

output "runtime_network" {
  description = "Runtime VPC network self link."
  value       = try(google_compute_network.runtime[0].self_link, null)
}

output "relayer_image" {
  description = "Effective relayer image deployed at runtime."
  value       = try(local.relayer_image_effective, null)
}

output "relayer_instance_group" {
  description = "Relayer regional managed instance group."
  value       = try(google_compute_region_instance_group_manager.relayer[0].instance_group, null)
}

output "relayer_region" {
  description = "Region where relayer instances run."
  value       = try(google_compute_region_instance_group_manager.relayer[0].region, null)
}

output "relayer_service_port" {
  description = "Relayer TCP port exposed by instances."
  value       = var.relayer_port
}

output "accumulator_image" {
  description = "Effective accumulator image deployed at runtime."
  value       = try(local.accumulator_image_effective, null)
}

output "accumulator_instance_name" {
  description = "Accumulator compute instance name."
  value       = try(google_compute_instance.accumulator[0].name, null)
}

output "accumulator_external_ip" {
  description = "Accumulator external IP address."
  value       = try(google_compute_instance.accumulator[0].network_interface[0].access_config[0].nat_ip, null)
}

output "market_monitor_image" {
  description = "Effective market-monitor image deployed at runtime."
  value       = try(local.market_monitor_image_effective, null)
}

output "market_monitor_instance_name" {
  description = "Market-monitor compute instance name."
  value       = try(google_compute_instance.market_monitor[0].name, null)
}

output "market_monitor_external_ip" {
  description = "Market-monitor external IP address."
  value       = try(google_compute_instance.market_monitor[0].network_interface[0].access_config[0].nat_ip, null)
}

output "funding_bridge_image" {
  description = "Effective funding-bridge image deployed at runtime."
  value       = try(local.funding_bridge_image_effective, null)
}

output "funding_bridge_instance_name" {
  description = "Funding-bridge compute instance name."
  value       = try(google_compute_instance.funding_bridge[0].name, null)
}

output "funding_bridge_external_ip" {
  description = "Funding-bridge external IP address."
  value       = try(google_compute_instance.funding_bridge[0].network_interface[0].access_config[0].nat_ip, null)
}

output "funding_bridge_service_port" {
  description = "Funding-bridge TCP port exposed by instances."
  value       = var.funding_bridge_port
}
