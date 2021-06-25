{ pkgs ? import (fetchTarball "https://github.com/NixOS/nixpkgs/archive/465e5fc8cd73eca36e6bc4a320fdac8bd50bb160.tar.gz") {} }:
with pkgs;
  mkShell {
    buildInputs = [ pkgconfig openssl rustup cargo-cross cmake ];
    CFG_DISABLE_CROSS_TESTS = "1";
  }
