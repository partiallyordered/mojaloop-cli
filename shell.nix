with import <nixpkgs> {};
mkShell {
  # buildInputs = [ pkgconfig openssl cmake zlib libgit2 ];
  buildInputs = [ pkgconfig openssl ];
  CFG_DISABLE_CROSS_TESTS = "1";
}
