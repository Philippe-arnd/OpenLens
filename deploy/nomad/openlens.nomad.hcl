# OpenLens as a Nomad job. Guide: docs/deploy/nomad.md.
#   nomad job run openlens.nomad.hcl
#   nomad job run -var image=ghcr.io/philippe-arnd/openlens@sha256:... openlens.nomad.hcl
#
# One allocation, on the client that has the `openlens-data` host volume:
# OpenLens is the only writer of its data, so an update stops the old
# allocation before it places the new one (no canary), and Nomad goes back to
# the last healthy version of the job if the new one does not become healthy.

variable "image" {
  type = string
  # A channel: :latest, or a minor such as :0.6 (never :edge). The updater
  # of the guide replaces it with the digest behind the channel, so that a
  # new image is a new version of the job.
  default = "ghcr.io/philippe-arnd/openlens:0.6"
}

variable "public_url" {
  type    = string
  default = "https://analytics.example.com"
}

job "openlens" {
  type = "service"

  update {
    max_parallel = 1
    health_check = "checks"
    # The first start of a new version copies the data to backups/upgrade/
    # before it listens.
    min_healthy_time  = "30s"
    healthy_deadline  = "20m"
    progress_deadline = "30m"
    auto_revert       = true
  }

  group "openlens" {
    count = 1

    volume "data" {
      type      = "host"
      source    = "openlens-data"
      read_only = false
    }

    network {
      port "http" {
        static = 8686
        to     = 8686
      }
    }

    restart {
      attempts = 3
      interval = "10m"
      delay    = "10s"
      mode     = "fail"
    }

    service {
      name     = "openlens"
      port     = "http"
      provider = "nomad"

      check {
        type     = "http"
        path     = "/healthz"
        interval = "15s"
        timeout  = "3s"

        check_restart {
          limit = 3
          # Checks are ignored while a new version copies the data.
          grace = "15m"
        }
      }
    }

    task "openlens" {
      driver = "docker"

      config {
        image      = var.image
        ports      = ["http"]
        force_pull = true
      }

      volume_mount {
        volume      = "data"
        destination = "/data"
      }

      # Optional settings: docs/deploy/README.md#optional-settings-d-016.
      # Secrets belong in a template from Nomad Variables (docs/deploy/nomad.md).
      env {
        OPENLENS_PUBLIC_URL = var.public_url
      }

      kill_timeout = "60s"

      resources {
        cpu = 1000
        # 1 GiB up to about 4 M events in 90 days
        # (docs/deploy/README.md#requirements). OpenLens reads this limit.
        memory = 1024
      }
    }
  }
}
