#!/bin/sh
# install.sh [CHANNEL]: install OpenLens as a hardened systemd service, or
# update an existing install. Guide: docs/deploy/systemd.md.
#
#   curl -fsSL https://raw.githubusercontent.com/Philippe-arnd/OpenLens/main/deploy/systemd/install.sh \
#     | sudo OPENLENS_PUBLIC_URL=https://analytics.example.com sh -s -- 0.6
#
# CHANNEL is `latest` or a minor such as `0.6` (patches of that minor only).
# Without it, the minor of the newest stable release.
#
# 1. Checks the machine: root, systemd, Linux x86_64 or aarch64 with glibc,
#    and the tools openlens-update needs (curl, jq, gh 2.49+ for the build
#    provenance).
# 2. Finds the newest stable release of the channel, and takes the systemd
#    files (openlens.service, openlens-update and its timer) from that
#    release's tag, so they match the binary.
# 3. Creates the `openlens` user and /etc/openlens/openlens.env (root only),
#    with OPENLENS_PUBLIC_URL when it is set. An existing file is kept.
# 4. Runs `openlens-update CHANNEL`, which downloads the release, checks
#    SHA256SUMS and the build provenance (Sigstore, signed by the release
#    workflow), installs it in /opt/openlens and, on an existing install,
#    restarts it and rolls back if it is not healthy.
# 5. Enables openlens.service and the daily openlens-update@CHANNEL.timer,
#    and prints where to find the first start's owner link.
#
# Running it again updates the files and the release; the data, the env file
# and the user stay.
set -eu

REPO=Philippe-arnd/OpenLens
RAW=https://raw.githubusercontent.com/$REPO
ROOT=/opt/openlens
UNITS=/etc/systemd/system
ENV_FILE=/etc/openlens/openlens.env

log() { printf 'install.sh: %s\n' "$*" >&2; }
die() { log "$*"; exit 1; }

# Newest stable tag (vX.Y.Z, no suffix) of channel $1.
resolve_tag() {
  json=$(curl -fsS --retry 3 -H 'Accept: application/vnd.github+json' \
    "https://api.github.com/repos/$REPO/releases?per_page=100") || return 1
  tags=$(printf '%s' "$json" | jq -r '.[] | select(.draft == false and .prerelease == false) | .tag_name') || return 1
  printf '%s\n' "$tags" | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' \
    | { if [ "$1" = latest ]; then cat; else grep -F "v$1." || true; fi; } \
    | sort -V | tail -n 1
}

# True when version $1 (vX.Y.Z) is older than $2.
older() {
  [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -n 1)" = "$1" ]
}

check_machine() {
  [ "$(id -u)" -eq 0 ] || die "run it as root (sudo sh install.sh)"
  [ "$(uname -s)" = Linux ] || die "Linux only: see docs/deploy/README.md for the Docker image"
  case "$(uname -m)" in
    x86_64 | amd64 | aarch64 | arm64) ;;
    *) die "no release for $(uname -m): the binaries are x86_64 and aarch64" ;;
  esac
  [ -d /run/systemd/system ] || die "systemd is not running here: use the Docker image (docs/deploy/docker.md)"
  if ldd --version 2>&1 | grep -qi musl; then
    die "the binaries need glibc (Debian 12, Ubuntu 22.04 or later): use the Docker image on musl systems"
  fi
  missing=""
  for tool in bash curl jq gh sha256sum tar flock sort useradd systemctl; do
    command -v "$tool" > /dev/null 2>&1 || missing="$missing $tool"
  done
  if [ -n "$missing" ]; then
    log "missing:$missing"
    log "on Debian or Ubuntu: apt install curl jq; gh 2.49+ from https://github.com/cli/cli/blob/trunk/docs/install_linux.md"
    exit 1
  fi
  gh attestation verify --help > /dev/null 2>&1 \
    || die "this gh has no 'attestation verify': install gh 2.49 or newer (see above)"
}

# Download the systemd files of release $1 and install them.
install_files() {
  work=$(mktemp -d)
  for f in openlens-update openlens.service openlens-update@.service openlens-update@.timer; do
    curl -fsSL --retry 3 -o "$work/$f" "$RAW/$1/deploy/systemd/$f" \
      || { rm -rf "$work"; die "cannot download $f of $1"; }
  done
  install -m 755 "$work/openlens-update" /usr/local/sbin/openlens-update
  for f in openlens.service openlens-update@.service openlens-update@.timer; do
    install -m 644 "$work/$f" "$UNITS/$f"
  done
  rm -rf "$work"
  systemctl daemon-reload
}

create_user_and_env() {
  if ! getent passwd openlens > /dev/null; then
    nologin=$(command -v nologin || echo /usr/sbin/nologin)
    useradd --system --home-dir /var/lib/openlens --shell "$nologin" openlens
    log "created the user openlens"
  fi
  install -d -m 700 /etc/openlens
  if [ ! -e "$ENV_FILE" ]; then
    (
      umask 077
      echo '# Optional settings and secrets: docs/deploy/README.md#optional-settings-d-016'
      if [ -n "${OPENLENS_PUBLIC_URL:-}" ]; then
        echo "OPENLENS_PUBLIC_URL=$OPENLENS_PUBLIC_URL"
      fi
    ) > "$ENV_FILE"
    log "wrote $ENV_FILE"
  elif [ -n "${OPENLENS_PUBLIC_URL:-}" ] && ! grep -q '^OPENLENS_PUBLIC_URL=' "$ENV_FILE"; then
    echo "OPENLENS_PUBLIC_URL=$OPENLENS_PUBLIC_URL" >> "$ENV_FILE"
    log "added OPENLENS_PUBLIC_URL to $ENV_FILE"
  fi
  chmod 600 "$ENV_FILE"
}

# Start (or restart, coming from an older install) and wait for /healthz.
# A new version first copies the data to backups/upgrade/, hence 15 minutes.
first_start() {
  systemctl enable openlens.service
  systemctl restart openlens.service
  i=0
  until OPENLENS_ADDR=127.0.0.1:8686 "$ROOT/current/openlens" healthcheck 2> /dev/null; do
    systemctl is-failed --quiet openlens.service && die "openlens failed to start: journalctl -u openlens"
    i=$((i + 1))
    [ "$i" -lt 450 ] || die "openlens did not answer on 127.0.0.1:8686 within 15 minutes: journalctl -u openlens"
    sleep 2
  done
  log "OpenLens is running on 127.0.0.1:8686; put your HTTPS reverse proxy in front (docs/deploy/docker.md#https-and-reverse-proxy)"
  log "owner link of a new install (single use, valid 1 hour): journalctl -u openlens | grep -i owner"
}

main() {
  check_machine
  channel=${1:-}
  if [ -z "$channel" ]; then
    newest=$(resolve_tag latest) || die "cannot read the releases from GitHub"
    [ -n "$newest" ] || die "no stable release yet"
    channel=$(printf '%s\n' "${newest#v}" | cut -d. -f1-2)
  fi
  printf '%s\n' "$channel" | grep -Eqx 'latest|[0-9]+\.[0-9]+' \
    || die "usage: install.sh [latest|X.Y] (for example 0.6)"

  tag=$(resolve_tag "$channel") || die "cannot read the releases from GitHub"
  [ -n "$tag" ] || die "no stable release in channel $channel"
  if older "$tag" v0.6.0; then
    die "$tag is not signed (signed releases start with 0.6.0): install it by hand, docs/deploy/systemd.md#update-by-hand-without-the-script"
  fi
  log "channel $channel: $tag"

  fresh=yes
  [ -L "$ROOT/current" ] && fresh=no
  install_files "$tag"
  create_user_and_env
  # Verifies, installs, and on an existing install restarts and rolls back.
  /usr/local/sbin/openlens-update "$channel"

  systemctl enable --now "openlens-update@$channel.timer"
  log "daily updates on channel $channel: systemctl list-timers 'openlens-update*'"
  if [ "$fresh" = yes ]; then
    first_start
  else
    systemctl enable openlens.service
    log "updated; OpenLens keeps its data in /var/lib/openlens"
  fi
}

main "$@"
