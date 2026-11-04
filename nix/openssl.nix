# Keep the application and source-build shell on the same supported LTS as
# release archives. Do not replace nixpkgs' OpenSSL throughout the tool closure.
pkgs:
let
  source = builtins.fromJSON (builtins.readFile ../packaging/openssl.json);
in pkgs.openssl_3_5.overrideAttrs (_old: {
  inherit (source) version;
  src = pkgs.fetchurl { inherit (source) url sha256; };
})
