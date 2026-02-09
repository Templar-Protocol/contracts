terraform {
  backend "gcs" {
    bucket = "templar-tfstate"
    prefix = "templar/gcp"
  }
}
