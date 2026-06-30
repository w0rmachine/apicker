{
  description = "apicker - A fast, scriptable file picker backend in Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils, ... }:
    let
      systemOutputs = flake-utils.lib.eachDefaultSystem (system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          packages.apicker = pkgs.rustPlatform.buildRustPackage {
            pname = "apicker";
            version = "0.1.0";
            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
            };
          };

          packages.default = self.packages.${system}.apicker;

          devShells.default = pkgs.mkShell {
            buildInputs = with pkgs; [
              cargo
              rustc
              rust-analyzer
              clippy
              rustfmt
            ];
          };
        }
      );
    in
    systemOutputs // {
      homeManagerModules.apicker = { config, lib, pkgs, ... }: {
        options.programs.apicker = {
          enable = lib.mkEnableOption "apicker";
          ignore_dirs = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [ "node_modules" ".venv" "target" ];
            description = "Directories to ignore when caching files";
          };
          settings = lib.mkOption {
            type = lib.types.attrs;
            default = {};
            description = "Configuration written to ~/.config/apicker/config.toml";
          };
        };
        config = lib.mkIf config.programs.apicker.enable {
          home.packages = [ self.packages.${pkgs.system}.apicker ];
          xdg.configFile."apicker/config.toml".text = 
            let 
              mergedSettings = config.programs.apicker.settings // {
                ignore_dirs = config.programs.apicker.ignore_dirs;
              };
              toTOML = (pkgs.formats.toml {}).generate "config.toml";
            in 
              builtins.readFile (toTOML mergedSettings);
        };
      };
      
      homeManagerModules.default = self.homeManagerModules.apicker;
    };
}
