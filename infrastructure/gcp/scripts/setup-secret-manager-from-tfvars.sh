#!/usr/bin/env bash
set -euo pipefail

if ! command -v gcloud >/dev/null 2>&1; then
  echo "gcloud is required but not found in PATH."
  exit 1
fi

PROJECT_ID=""
TFVARS_PATH="infrastructure/gcp/terraform.tfvars"
RUNTIME_SA_EMAIL=""

usage() {
  cat <<'EOM'
Usage:
  setup-secret-manager-from-tfvars.sh --project PROJECT_ID [--tfvars PATH] [--runtime-sa SA_EMAIL]

Options:
  --project     GCP project ID (required)
  --tfvars      Path to terraform.tfvars (default: infrastructure/gcp/terraform.tfvars)
  --runtime-sa  Runtime service account email to grant Secret Accessor on each secret
EOM
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --project)
      val="${2:-}"
      if [[ -z "${val}" || "${val}" == --* ]]; then
        echo "Missing value for --project." >&2
        usage >&2
        exit 1
      fi
      PROJECT_ID="${val}"
      shift 2
      ;;
    --tfvars)
      val="${2:-}"
      if [[ -z "${val}" || "${val}" == --* ]]; then
        echo "Missing value for --tfvars." >&2
        usage >&2
        exit 1
      fi
      TFVARS_PATH="${val}"
      shift 2
      ;;
    --runtime-sa)
      val="${2:-}"
      if [[ -z "${val}" || "${val}" == --* ]]; then
        echo "Missing value for --runtime-sa." >&2
        usage >&2
        exit 1
      fi
      RUNTIME_SA_EMAIL="${val}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1"
      exit 1
      ;;
  esac
done

if [[ -z "${PROJECT_ID}" ]]; then
  echo "--project is required."
  exit 1
fi

if [[ ! -f "${TFVARS_PATH}" ]]; then
  echo "tfvars file not found: ${TFVARS_PATH}"
  exit 1
fi

readarray -t SECRETS < <(
  awk '
    BEGIN {
      in_map = 0
    }
    /^[[:space:]]*(relayer_secret_env|accumulator_secret_env|market_monitor_secret_env|funding_bridge_secret_env)[[:space:]]*=[[:space:]]*\{/ {
      in_map = 1
      next
    }
    in_map == 1 && /^[[:space:]]*}/ {
      in_map = 0
      next
    }
    in_map == 1 {
      if (match($0, /^[[:space:]]*[A-Za-z_][A-Za-z0-9_]*[[:space:]]*=[[:space:]]*"([^"]+)"/, m)) {
        print m[1]
      }
    }
  ' "${TFVARS_PATH}" | sort -u
)

if [[ ${#SECRETS[@]} -eq 0 ]]; then
  echo "No secrets found in *_secret_env maps in ${TFVARS_PATH}."
  exit 0
fi

echo "Found ${#SECRETS[@]} unique secrets in ${TFVARS_PATH}:"
for secret_id in "${SECRETS[@]}"; do
  echo "  - ${secret_id}"
done

gcloud services enable secretmanager.googleapis.com --project "${PROJECT_ID}"

ensure_secret() {
  local secret_id="$1"
  if gcloud secrets describe "${secret_id}" --project "${PROJECT_ID}" >/dev/null 2>&1; then
    echo "Secret exists: ${secret_id}"
  else
    echo "Creating secret: ${secret_id}"
    gcloud secrets create "${secret_id}" \
      --project "${PROJECT_ID}" \
      --replication-policy "automatic"
  fi
}

for secret_id in "${SECRETS[@]}"; do
  ensure_secret "${secret_id}"

  read -r -s -p "Enter value for ${secret_id} (leave empty to skip version add): " secret_value
  echo

  if [[ -z "${secret_value}" ]]; then
    echo "Skipped new version for ${secret_id}."
    continue
  fi

  printf '%s' "${secret_value}" | gcloud secrets versions add "${secret_id}" \
    --project "${PROJECT_ID}" \
    --data-file=-
  echo "Added latest version for ${secret_id}."
done

if [[ -n "${RUNTIME_SA_EMAIL}" ]]; then
  for secret_id in "${SECRETS[@]}"; do
    gcloud secrets add-iam-policy-binding "${secret_id}" \
      --project "${PROJECT_ID}" \
      --member "serviceAccount:${RUNTIME_SA_EMAIL}" \
      --role "roles/secretmanager.secretAccessor" >/dev/null
    echo "Granted accessor on ${secret_id} to ${RUNTIME_SA_EMAIL}."
  done
else
  echo "No --runtime-sa provided; skipped IAM bindings."
fi

echo "Done."
