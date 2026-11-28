# NixOS module: services.openlens. Guide: docs/deploy/nixos.md.
# The service mirrors deploy/systemd/openlens.service: its own user, data in
# /var/lib/openlens, a 1 GiB memory cap, the same sandbox.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.openlens;
in
{
  options.services.openlens = {
    enable = lib.mkEnableOption "OpenLens, self-hosted product analytics";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
      description = "The OpenLens package, built from this source tree by default.";
    };

    address = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1:8686";
      description = "Where OpenLens listens (OPENLENS_ADDR). Put a reverse proxy with HTTPS in front.";
    };

    publicUrl = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "https://analytics.example.com";
      description = "The public HTTPS URL (OPENLENS_PUBLIC_URL), for login links and emails.";
    };

    environmentFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      example = "/run/secrets/openlens.env";
      description = ''
        A file of OPENLENS_* variables kept out of the Nix store (secrets such
        as OPENLENS_SMTP_URL). The list is in docs/deploy/README.md.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.openlens = {
      isSystemUser = true;
      group = "openlens";
      home = "/var/lib/openlens";
    };
    users.groups.openlens = { };

    environment.systemPackages = [ cfg.package ];

    systemd.services.openlens = {
      description = "OpenLens product analytics";
      documentation = [ "https://github.com/Philippe-arnd/OpenLens/blob/main/docs/deploy/nixos.md" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];

      environment = {
        OPENLENS_DATA_DIR = "/var/lib/openlens";
        OPENLENS_ADDR = cfg.address;
      }
      // lib.optionalAttrs (cfg.publicUrl != null) { OPENLENS_PUBLIC_URL = cfg.publicUrl; };

      serviceConfig = {
        Type = "simple";
        User = "openlens";
        Group = "openlens";
        StateDirectory = "openlens";
        StateDirectoryMode = "0750";
        EnvironmentFile = lib.mkIf (cfg.environmentFile != null) cfg.environmentFile;
        ExecStart = lib.getExe' cfg.package "openlens";
        Restart = "on-failure";
        RestartSec = 5;
        # The first start after an update copies the data first (backups/upgrade/).
        TimeoutStartSec = "infinity";
        TimeoutStopSec = 60;
        LimitNOFILE = 65536;
        UMask = "0077";

        # 1 GiB covers about 4 M events in 90 days; query memory stops at
        # 768 MiB on any host (D-097,
        # docs/deploy/README.md#requirements). OpenLens reads this limit.
        MemoryMax = lib.mkDefault "1G";

        # Sandbox, as deploy/systemd/openlens.service. /proc stays readable:
        # OpenLens reads its memory limit there.
        NoNewPrivileges = true;
        CapabilityBoundingSet = "";
        AmbientCapabilities = "";
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectProc = "invisible";
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        ProtectClock = true;
        ProtectHostname = true;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
        ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        RemoveIPC = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [
          "@system-service"
          "~@privileged"
        ];
        SystemCallErrorNumber = "EPERM";
      };
    };
  };
}
