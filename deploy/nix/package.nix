# OpenLens built from this source tree. Guide: docs/deploy/nixos.md.
# Needs a nixpkgs with Rust 1.88 or newer (NixOS 25.11 and later).
{ lib, rustPlatform }:

let
  workspace = (lib.importTOML ../../Cargo.toml).workspace.package;
in
rustPlatform.buildRustPackage {
  pname = "openlens";
  inherit (workspace) version;

  # The whole repository: the binary embeds the SDK bundles (sdk/js/dist)
  # and some docs. Local build output and notebooks are left out.
  src = lib.cleanSourceWith {
    src = lib.cleanSource ../..;
    filter =
      path: _type:
      !(builtins.elem (baseNameOf path) [
        "target"
        "data"
        "plan"
        "log"
        "node_modules"
        ".claude"
      ]);
  };

  # Every dependency comes from crates.io and is pinned by Cargo.lock: no
  # hash to update per release.
  cargoLock.lockFile = ../../Cargo.lock;
  cargoBuildFlags = [
    "-p"
    "openlens"
    "-p"
    "openlens-cli"
  ];
  # The test suite runs in this repository's CI.
  doCheck = false;

  meta = {
    description = "Self-hosted product analytics, one binary and one data directory";
    homepage = "https://github.com/Philippe-arnd/OpenLens";
    license = lib.licenses.agpl3Only;
    mainProgram = "openlens";
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
    ];
  };
}
