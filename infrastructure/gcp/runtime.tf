locals {
  runtime_default_image = "${local.artifact_registry_location}-docker.pkg.dev/${var.project_id}/${var.artifact_registry_repository_id}/templar:latest"

  relayer_image_effective        = coalesce(var.relayer_image, local.runtime_default_image)
  accumulator_image_effective    = coalesce(var.accumulator_image, local.runtime_default_image)
  market_monitor_image_effective = coalesce(var.market_monitor_image, local.runtime_default_image)
  funding_bridge_image_effective = coalesce(var.funding_bridge_image, local.runtime_default_image)

  relayer_zones_effective = length(var.relayer_zones) > 0 ? var.relayer_zones : ["${var.region}-b", "${var.region}-c"]
  accumulator_zone_effective = coalesce(
    var.accumulator_zone,
    local.relayer_zones_effective[0]
  )
  market_monitor_zone_effective = coalesce(
    var.market_monitor_zone,
    local.relayer_zones_effective[0]
  )
  funding_bridge_zone_effective = coalesce(
    var.funding_bridge_zone,
    local.relayer_zones_effective[0]
  )

  relayer_env_effective = merge(
    {
      PORT = tostring(var.relayer_port)
    },
    var.relayer_env
  )

  relayer_env_file = join("\n", [
    for key in sort(keys(local.relayer_env_effective)) : "${key}=${replace(replace(local.relayer_env_effective[key], "\\n", ""), "\\r", "")}"
  ])

  accumulator_env_effective = merge(
    {
      INTERVAL        = "43200"
      STATIC_INTERVAL = "86400"
    },
    var.accumulator_env
  )
  market_monitor_env_effective = merge(
    {
      RUST_LOG = "info,templar_market_monitor=info"
    },
    var.market_monitor_env
  )
  funding_bridge_env_effective = merge(
    {
      PORT = tostring(var.funding_bridge_port)
    },
    var.funding_bridge_env
  )

  accumulator_env_file = join("\n", [
    for key in sort(keys(local.accumulator_env_effective)) : "${key}=${replace(replace(local.accumulator_env_effective[key], "\\n", ""), "\\r", "")}"
  ])
  market_monitor_env_file = join("\n", [
    for key in sort(keys(local.market_monitor_env_effective)) : "${key}=${replace(replace(local.market_monitor_env_effective[key], "\\n", ""), "\\r", "")}"
  ])
  funding_bridge_env_file = join("\n", [
    for key in sort(keys(local.funding_bridge_env_effective)) : "${key}=${replace(replace(local.funding_bridge_env_effective[key], "\\n", ""), "\\r", "")}"
  ])
}

resource "google_service_account" "runtime" {
  count = var.enable_runtime ? 1 : 0

  project      = var.project_id
  account_id   = var.runtime_service_account_id
  display_name = var.runtime_service_account_display_name
}

resource "google_project_iam_member" "runtime_sa_roles" {
  for_each = var.enable_runtime ? toset(var.runtime_service_account_project_roles) : toset([])

  project = var.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.runtime[0].email}"
}

resource "google_compute_network" "runtime" {
  count = var.enable_runtime ? 1 : 0

  project                 = var.project_id
  name                    = var.runtime_network_name
  auto_create_subnetworks = false

  depends_on = [google_project_service.required]
}

resource "google_compute_subnetwork" "runtime" {
  count = var.enable_runtime ? 1 : 0

  project       = var.project_id
  name          = var.runtime_subnet_name
  region        = var.region
  ip_cidr_range = var.runtime_subnet_cidr
  network       = google_compute_network.runtime[0].id
}

resource "google_compute_firewall" "runtime_ssh" {
  count = var.enable_runtime && length(var.admin_source_ranges) > 0 ? 1 : 0

  project = var.project_id
  name    = "templar-runtime-allow-ssh"
  network = google_compute_network.runtime[0].name

  direction     = "INGRESS"
  source_ranges = var.admin_source_ranges
  target_tags   = ["templar-runtime"]

  allow {
    protocol = "tcp"
    ports    = ["22"]
  }
}

resource "google_compute_firewall" "relayer_ingress" {
  count = var.enable_runtime && var.relayer_enabled ? 1 : 0

  project = var.project_id
  name    = "templar-relayer-allow-client"
  network = google_compute_network.runtime[0].name

  direction     = "INGRESS"
  source_ranges = var.relayer_allowed_source_ranges
  target_tags   = ["templar-relayer", "templar-runtime"]

  allow {
    protocol = "tcp"
    ports    = [tostring(var.relayer_port)]
  }
}

resource "google_compute_firewall" "relayer_healthcheck" {
  count = var.enable_runtime && var.relayer_enabled ? 1 : 0

  project = var.project_id
  name    = "templar-relayer-allow-healthcheck"
  network = google_compute_network.runtime[0].name

  direction     = "INGRESS"
  source_ranges = ["130.211.0.0/22", "35.191.0.0/16"]
  target_tags   = ["templar-relayer", "templar-runtime"]

  allow {
    protocol = "tcp"
    ports    = [tostring(var.relayer_port)]
  }
}

resource "google_compute_firewall" "funding_bridge_ingress" {
  count = var.enable_runtime && var.funding_bridge_enabled ? 1 : 0

  project = var.project_id
  name    = "templar-funding-bridge-allow-client"
  network = google_compute_network.runtime[0].name

  direction     = "INGRESS"
  source_ranges = var.funding_bridge_allowed_source_ranges
  target_tags   = ["templar-funding-bridge", "templar-runtime"]

  allow {
    protocol = "tcp"
    ports    = [tostring(var.funding_bridge_port)]
  }
}

resource "google_compute_health_check" "relayer" {
  count = var.enable_runtime && var.relayer_enabled ? 1 : 0

  project = var.project_id
  name    = "templar-relayer-tcp"

  check_interval_sec  = 10
  timeout_sec         = 5
  healthy_threshold   = 2
  unhealthy_threshold = 3

  tcp_health_check {
    port = var.relayer_port
  }
}

resource "google_compute_instance_template" "relayer" {
  count = var.enable_runtime && var.relayer_enabled ? 1 : 0

  project      = var.project_id
  name_prefix  = "templar-relayer-"
  machine_type = var.relayer_machine_type
  tags         = ["templar-relayer", "templar-runtime"]

  disk {
    auto_delete  = true
    boot         = true
    source_image = var.runtime_source_image
    disk_size_gb = var.relayer_disk_size_gb
    disk_type    = "pd-balanced"
  }

  network_interface {
    subnetwork = google_compute_subnetwork.runtime[0].id

    access_config {}
  }

  metadata_startup_script = templatefile("${path.module}/templates/daemon-startup.sh.tftpl", {
    registry_host  = "${local.artifact_registry_location}-docker.pkg.dev"
    image          = local.relayer_image_effective
    container_name = var.relayer_container_name
    binary_path    = "/app/bin/templar-relayer"
    env_file       = local.relayer_env_file
  })

  scheduling {
    automatic_restart   = true
    on_host_maintenance = "MIGRATE"
    preemptible         = false
  }

  service_account {
    email  = google_service_account.runtime[0].email
    scopes = ["https://www.googleapis.com/auth/cloud-platform"]
  }

  lifecycle {
    create_before_destroy = true
  }

  depends_on = [google_project_iam_member.runtime_sa_roles]
}

resource "google_compute_region_instance_group_manager" "relayer" {
  count = var.enable_runtime && var.relayer_enabled ? 1 : 0

  project            = var.project_id
  name               = "templar-relayer-rigm"
  region             = var.region
  base_instance_name = "templar-relayer"
  target_size        = var.relayer_instance_count

  distribution_policy_zones = local.relayer_zones_effective

  version {
    instance_template = google_compute_instance_template.relayer[0].id
    name              = "primary"
  }

  named_port {
    name = "http"
    port = var.relayer_port
  }

  auto_healing_policies {
    health_check      = google_compute_health_check.relayer[0].id
    initial_delay_sec = 180
  }

  update_policy {
    type                  = "PROACTIVE"
    minimal_action        = "REPLACE"
    max_surge_fixed       = length(local.relayer_zones_effective)
    max_unavailable_fixed = 0
    replacement_method    = "SUBSTITUTE"
  }
}

resource "google_compute_region_autoscaler" "relayer" {
  count = var.enable_runtime && var.relayer_enabled && var.relayer_autoscaling_enabled ? 1 : 0

  project = var.project_id
  name    = "templar-relayer-autoscaler"
  region  = var.region
  target  = google_compute_region_instance_group_manager.relayer[0].id

  autoscaling_policy {
    min_replicas    = var.relayer_min_replicas
    max_replicas    = var.relayer_max_replicas
    cooldown_period = 90

    cpu_utilization {
      target = var.relayer_cpu_target
    }
  }
}

resource "google_compute_instance" "accumulator" {
  count = var.enable_runtime && var.accumulator_enabled ? 1 : 0

  project      = var.project_id
  zone         = local.accumulator_zone_effective
  name         = "templar-accumulator"
  machine_type = var.accumulator_machine_type
  tags         = ["templar-runtime", "templar-accumulator"]

  boot_disk {
    auto_delete = true

    initialize_params {
      image = var.runtime_source_image
      size  = var.accumulator_disk_size_gb
      type  = "pd-balanced"
    }
  }

  network_interface {
    subnetwork = google_compute_subnetwork.runtime[0].id

    access_config {}
  }

  metadata_startup_script = templatefile("${path.module}/templates/daemon-startup.sh.tftpl", {
    registry_host  = "${local.artifact_registry_location}-docker.pkg.dev"
    image          = local.accumulator_image_effective
    container_name = var.accumulator_container_name
    binary_path    = "/app/bin/accumulator"
    env_file       = local.accumulator_env_file
  })

  service_account {
    email  = google_service_account.runtime[0].email
    scopes = ["https://www.googleapis.com/auth/cloud-platform"]
  }

  scheduling {
    automatic_restart   = true
    on_host_maintenance = "MIGRATE"
    preemptible         = false
  }

  allow_stopping_for_update = true

  depends_on = [google_project_iam_member.runtime_sa_roles]
}

resource "google_compute_instance" "market_monitor" {
  count = var.enable_runtime && var.market_monitor_enabled ? 1 : 0

  project      = var.project_id
  zone         = local.market_monitor_zone_effective
  name         = "templar-market-monitor"
  machine_type = var.market_monitor_machine_type
  tags         = ["templar-runtime", "templar-market-monitor"]

  boot_disk {
    auto_delete = true

    initialize_params {
      image = var.runtime_source_image
      size  = var.market_monitor_disk_size_gb
      type  = "pd-balanced"
    }
  }

  network_interface {
    subnetwork = google_compute_subnetwork.runtime[0].id

    access_config {}
  }

  metadata_startup_script = templatefile("${path.module}/templates/daemon-startup.sh.tftpl", {
    registry_host  = "${local.artifact_registry_location}-docker.pkg.dev"
    image          = local.market_monitor_image_effective
    container_name = var.market_monitor_container_name
    binary_path    = "/app/bin/market-monitor"
    env_file       = local.market_monitor_env_file
  })

  service_account {
    email  = google_service_account.runtime[0].email
    scopes = ["https://www.googleapis.com/auth/cloud-platform"]
  }

  scheduling {
    automatic_restart   = true
    on_host_maintenance = "MIGRATE"
    preemptible         = false
  }

  allow_stopping_for_update = true

  depends_on = [google_project_iam_member.runtime_sa_roles]
}

resource "google_compute_instance" "funding_bridge" {
  count = var.enable_runtime && var.funding_bridge_enabled ? 1 : 0

  project      = var.project_id
  zone         = local.funding_bridge_zone_effective
  name         = "templar-funding-bridge"
  machine_type = var.funding_bridge_machine_type
  tags         = ["templar-runtime", "templar-funding-bridge"]

  boot_disk {
    auto_delete = true

    initialize_params {
      image = var.runtime_source_image
      size  = var.funding_bridge_disk_size_gb
      type  = "pd-balanced"
    }
  }

  network_interface {
    subnetwork = google_compute_subnetwork.runtime[0].id

    access_config {}
  }

  metadata_startup_script = templatefile("${path.module}/templates/daemon-startup.sh.tftpl", {
    registry_host  = "${local.artifact_registry_location}-docker.pkg.dev"
    image          = local.funding_bridge_image_effective
    container_name = var.funding_bridge_container_name
    binary_path    = "/app/bin/funding-bridge"
    env_file       = local.funding_bridge_env_file
  })

  service_account {
    email  = google_service_account.runtime[0].email
    scopes = ["https://www.googleapis.com/auth/cloud-platform"]
  }

  scheduling {
    automatic_restart   = true
    on_host_maintenance = "MIGRATE"
    preemptible         = false
  }

  allow_stopping_for_update = true

  depends_on = [google_project_iam_member.runtime_sa_roles]
}
