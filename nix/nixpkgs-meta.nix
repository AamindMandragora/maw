# what maw's scaffolder needs from one nixpkgs package, as plain data; nothing is built
{ nixpkgs, attr }:
let
  pkgs = import nixpkgs {
    config.allowUnfree = true;
    overlays = [ ];
  };
  lib = pkgs.lib;
  pkg = lib.getAttrFromPath (lib.splitString "." attr) pkgs;
  src = pkg.src or { };

  # dependency names, the way nixpkgs calls them
  names = deps: map (dep: dep.pname or (lib.getName dep)) (builtins.filter lib.isDerivation deps);
  native = names (pkg.nativeBuildInputs or [ ]);

  # meta.homepage and meta.license may be one value or a list
  first = value: if builtins.isList value then builtins.head value else value;
  licenses = map (license: license.spdxId or license.shortName or "unknown") (lib.toList (pkg.meta.license or [ ]));

  # the builder, from what each nixpkgs builder leaves behind on the derivation
  builder =
    if pkg ? vendorHash || pkg ? goModules then
      "go"
    else if pkg ? cargoDeps then
      "cargo"
    else if pkg ? pythonModule then
      "python3-module"
    else if builtins.elem "meson" native then
      "meson"
    else if builtins.elem "cmake" native then
      "cmake"
    else
      "gnu-configure";
in
{
  inherit builder licenses;
  pname = pkg.pname or (lib.getName pkg);
  version = pkg.version or "";
  description = pkg.meta.description or "";
  homepage = first (pkg.meta.homepage or "");
  urls = src.urls or (if src ? url then [ src.url ] else [ ]);
  nativeBuildInputs = native;
  buildInputs = names ((pkg.buildInputs or [ ]) ++ (pkg.propagatedBuildInputs or [ ]));
  subPackages = pkg.subPackages or [ ];
}
