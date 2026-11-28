# Deploy on NixOS

The general guide is [README.md](README.md): requirements, release channels, backups and settings. The files are [`flake.nix`](../../flake.nix) and [`deploy/nix/`](../../deploy/nix/):

| File | What it is |
| --- | --- |
| `deploy/nix/package.nix` | OpenLens built from the source of the release you pin (`openlens` and `openlens-cli`) |
| `deploy/nix/module.nix` | the NixOS module `services.openlens`: the service of [systemd.md](systemd.md), its user and its sandbox |
| `flake.nix` | both, as `packages.<system>.default` and `nixosModules.default` |

The package needs Rust 1.88 or newer: NixOS 25.11 or later. It is built on your machine (or your binary cache): the first build takes a while, like any Rust program of this size.

## Install

With flakes, pin a release tag in your system flake:

```nix
{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
  inputs.openlens.url = "github:Philippe-arnd/OpenLens/v0.6.0";
  inputs.openlens.inputs.nixpkgs.follows = "nixpkgs";

  outputs = { nixpkgs, openlens, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [ ./configuration.nix openlens.nixosModules.default ];
    };
  };
}
```

Without flakes, import the module from the release's archive:

```nix
imports = [ "${builtins.fetchTarball "https://github.com/Philippe-arnd/OpenLens/archive/v0.6.0.tar.gz"}/deploy/nix/module.nix" ];
```

Then, in `configuration.nix`:

```nix
services.openlens = {
  enable = true;
  publicUrl = "https://analytics.example.com";
  # Secrets (OPENLENS_SMTP_URL, OPENLENS_S3_SECRET_ACCESS_KEY, ...) stay out of the Nix store:
  # environmentFile = "/run/secrets/openlens.env";
};

# HTTPS in front, for example with Caddy:
services.caddy.enable = true;
services.caddy.virtualHosts."analytics.example.com".extraConfig = "reverse_proxy 127.0.0.1:8686";
```

```sh
sudo nixos-rebuild switch
journalctl -u openlens | grep -i owner    # first start: a single-use owner link, valid 1 hour
```

| Option | Default | What it sets |
| --- | --- | --- |
| `enable` | `false` | the `openlens` service, user and group |
| `package` | built from the module's source | the OpenLens package |
| `address` | `127.0.0.1:8686` | `OPENLENS_ADDR` |
| `publicUrl` | none | `OPENLENS_PUBLIC_URL` |
| `environmentFile` | none | a file of `OPENLENS_*` variables, read by systemd |

Other settings ([the list](README.md#optional-settings-d-016)) go in `systemd.services.openlens.environment`, or in the environment file when they are secret. The service is the unit of [systemd.md](systemd.md#what-the-unit-sets): data in `/var/lib/openlens`, its own user, the same sandbox, and `MemoryMax=1G`, which covers about 4 M events in 90 days.

## Updates

The system's own upgrades update OpenLens: move the tag (`inputs.openlens.url = "github:Philippe-arnd/OpenLens/v0.6.1"`, or the archive URL), then `nixos-rebuild switch`. Pin a release tag, never `main`, which is the `edge` channel. Patches of a minor (`v0.6.x`) bring fixes without behaviour changes; before the next minor, read its **Changed** section in the [CHANGELOG](../../CHANGELOG.md). The dashboard footer says *Update available* when a newer release is out.

On start, a new version backs up its data to `backups/upgrade/` and refuses data written by a newer release ([README.md](README.md#updates-without-anyone)).

### Roll back

`sudo nixos-rebuild switch --rollback` (or boot the previous generation) brings the previous OpenLens back. If the log then says *this data was written by OpenLens X (data format N)*, the newer release changed the data format: put back the copy it made before it started with the steps of [systemd.md](systemd.md#roll-back).

## Backups

From the dashboard (*Projects → Storage and backups*), or a timer or cron line as the `openlens` user:

```sh
sudo -u openlens env OPENLENS_DATA_DIR=/var/lib/openlens openlens backup --into /var/lib/openlens/backups/daily --keep 7
```

Then copy that directory off the machine. Details in [README.md](README.md#backups).
