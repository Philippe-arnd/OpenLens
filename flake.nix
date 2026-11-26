# OpenLens for Nix: the package and the NixOS module services.openlens.
# Guide: docs/deploy/nixos.md. The files are in deploy/nix/.
{
  description = "OpenLens, self-hosted product analytics";

  # Rust 1.88 or newer.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        openlens = pkgs.callPackage ./deploy/nix/package.nix { };
        default = openlens;
      });

      # Builds the package with the system's own nixpkgs.
      nixosModules.default = ./deploy/nix/module.nix;
      nixosModules.openlens = self.nixosModules.default;
    };
}
