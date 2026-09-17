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
          root_dir = lib.mkOption {
            type = lib.types.str;
            default = config.home.homeDirectory;
            description = "Root directory scanned for the file cache.";
          };
          cache_ttl_seconds = lib.mkOption {
            type = lib.types.int;
            default = 3600;
            description = "Maximum age of the file cache before it is rebuilt. Set to 0 to disable TTL-based rebuilds.";
          };
          desktop_exclude = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [];
            description = "Escape-hatch list of desktop-file IDs (e.g. \"foo.desktop\") to hide, matched case-insensitively. Normal visibility follows the XDG Desktop Entry specification.";
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
                root_dir = config.programs.apicker.root_dir;
                cache_ttl_seconds = config.programs.apicker.cache_ttl_seconds;
                desktop_exclude = config.programs.apicker.desktop_exclude;
              };
              toTOML = (pkgs.formats.toml {}).generate "config.toml";
            in 
              builtins.readFile (toTOML mergedSettings);
        };
      };
      
      homeManagerModules.default = self.homeManagerModules.apicker;
    };
}
