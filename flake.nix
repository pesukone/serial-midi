{
  description = "Rustup flake";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overrides = (fromTOML (builtins.readFile ./rust-toolchain.toml));
        pkgs = nixpkgs.legacyPackages.${system};
      in
      {
        devShells.default = pkgs.mkShell {
          strictDeps = true;
          nativeBuildInputs = with pkgs; [
            rustup
            rustPlatform.bindgenHook
            jdk25
          ];
          buildInputs = [ ];

          RUSTC_VERSION = overrides.toolchain.channel;

          shellHook = ''
            export PATH="''${CARGO_HOME:~/.cargo}/bin":"$PATH"
            export PATH="''${RUSTUP_HOME:~/.rustup}/toolchains/$RUSTC_VERSION-${pkgs.stdenv.hostPlatform.rust.rustcTarget}/bin":"$PATH"
          '';
        };
      }
    );
}
