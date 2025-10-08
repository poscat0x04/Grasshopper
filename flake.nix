{
  inputs = {
    nixpkgs.url = "git+ssh://git@github.com/NixOS/nixpkgs?ref=nixos-24.05&shallow=1";
    flake-utils.url = "git+ssh://git@github.com/poscat0x04/flake-utils?shallow=1";
    nix-filter.url = "git+ssh://git@github.com/numtide/nix-filter?shallow=1";
  };

  outputs =
    { self
    , nixpkgs
    , flake-utils
    , nix-filter
    }:
    flake-utils.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; overlays = [ self.overlay ]; };
      in {
        packages = rec {
          inherit (pkgs) grhp;
          default = grhp;
        };
      }
    ) // {
      nixosModules.grhp =
        { config, lib, pkgs, ... }:
        let
          cfg = config.services.grhp;
        in {
          options.services.grhp = with lib; {
            enable = mkEnableOption "Grasshopper service";

            args = mkOption {
              type = types.str;
            };
          };
          config = lib.mkIf cfg.enable {
            systemd.services.grhp = {
              after = [ "network-online.target" ];
              wantedBy = [ "multi-user.target" ];
              serviceConfig = {
                DynamicUser = true;
                User = "grhp";
                Group = "grhp";
                AmbientCapabilities = [ "CAP_NET_BIND_SERVICE" ];
                CapabilityBoundingSet = [ "CAP_NET_BIND_SERVICE" ];
                Restart = "on-failure";
                RestartSec = "3s";
                ExecStart = "${pkgs.grhp}/bin/grhp ${cfg.args}";
              };
            };
          };
        };
      overlay = final: prev: {
        grhp =
          let
            cargo-toml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          in with final.rustPlatform; buildRustPackage {
            pname = cargo-toml.package.name;
            version = cargo-toml.package.version;

            src = nix-filter.lib {
              root = ./.;
              include = [
                ./src
                ./Cargo.toml
                ./Cargo.lock
              ];
            };
          };
      };
    };
}
