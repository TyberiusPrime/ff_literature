{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/release-25.11";
    utils.url = "github:numtide/flake-utils";
    naersk.url = "github:nmattia/naersk";
    naersk.inputs.nixpkgs.follows = "nixpkgs";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      self,
      nixpkgs,
      utils,
      naersk,
      rust-overlay,
    }:
    utils.lib.eachDefaultSystem (
      system:
      let
        #pkgs = nixpkgs.legacyPackages."${system}";
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        rust = pkgs.rust-bin.stable."1.93.1".default.override {
          targets = [ "x86_64-unknown-linux-musl" ];
          extensions = [
            "llvm-tools-preview"
            "rust-analyzer"
          ];
        };

        # Override the version used in naersk
        naersk-lib = naersk.lib."${system}".override {
          cargo = rust;
          rustc = rust;
        };

        bacon = pkgs.bacon;

      in
      rec {
        # `nix build`
        packages.ff_literature = (
          naersk-lib.buildPackage {
            pname = "ff_literature";
            root = ./.;
            nativeBuildInputs = with pkgs; [
              pkg-config
            ];
            buildInputs = with pkgs; [
              openssl
              poppler_utils
            ];
            release = true;
            CARGO_PROFILE_RELEASE_debug = "0";
            COMMIT_HASH = self.rev or (pkgs.lib.removeSuffix "-dirty" self.dirtyRev or "unknown-not-in-git");
            NIX_RAPIDGZIP = "${pkgs.rapidgzip}/bin/rapidgzip";
          }
        );
        packages.check = naersk-lib.buildPackage {
          src = ./.;
          mode = "check";
          name = "ff_literature";
          nativeBuildInputs = with pkgs; [
          ];
          buildInputs = with pkgs; [ ];
        };
        packages.test = naersk-lib.buildPackage {
          # not using naersk test mode, it eats the binaries, we need that binary
          pname = "ff_literature";
          root = ./.;
          nativeBuildInputs = with pkgs; [
          ];
          buildInputs = with pkgs; [ ];
          release = true;
          CARGO_PROFILE_RELEASE_debug = "0";
          COMMIT_HASH = self.rev or (pkgs.lib.removeSuffix "-dirty" self.dirtyRev or "unknown-not-in-git");
          RUST_LOG = "trace";
        };

        defaultPackage = packages.ff_literature;

        # `nix run`
        apps.ff_literature = utils.lib.mkApp { drv = packages.ff_lookup; };
        defaultApp = apps.ff_literature;

        # `nix develop`
        devShell = pkgs.mkShell {
          COMMIT_HASH = self.rev or (pkgs.lib.removeSuffix "-dirty" self.dirtyRev or "unknown-not-in-git");
          # we only link with mold in our dev environment for build speed. CI can use the old school rust linker
          shellHook = ''
            export ff_literature_DIR="./lookup"
          '';
          # supply the specific rust version
          nativeBuildInputs = [
            bacon
            pkgs.poppler_utils
            pkgs.openssl
            pkgs.pkg-config
            # pkgs.bash
            # pkgs.aflplusplus
            # cargo-afl
            # pkgs.cargo-audit
            # pkgs.cargo-bloat
            # pkgs.cargo-crev
            # pkgs.cargo-deny
            # pkgs.cargo-features-manager
            # pkgs.cargo-flamegraph
            # pkgs.cargo-insta
            # pkgs.cargo-license
            # pkgs.cargo-llvm-cov
            # pkgs.cargo-llvm-lines
            # pkgs.lcov
            # pkgs.cargo-machete
            # pkgs.cargo-mutants
            # pkgs.cargo-nextest
            # pkgs.cargo-outdated
            # pkgs.cargo-shear
            #pkgs.cargo-udeps
            # pkgs.cargo-vet
            # pkgs.cmake
            # pkgs.gcc
            # pkgs.gnumake
            # pkgs.git
            # pkgs.hugo
            # pkgs.jq
            # pkgs.mold
            # pkgs.openssl
            # pkgs.pkg-config
            # pkgs.samply
            # (pkgs.python3.withPackages (
            #   ps: with ps; [
            #     scipy
            #     pysam
            #     pandas
            #     toml
            #   ]
            # ))
            # pkgs.rapidgzip
            # pkgs.which
            # pkgs.ripgrep
            # #rust.rust-analyzer
            # pkgs.shellcheck
            rust
          ];
        };
      }
    );
}
# {
