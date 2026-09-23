{ dotfiles }:
let
  lib = import ./lib.nix;

  # config.nix may be a plain attrset or a function of lib
  userConfig = import (dotfiles + "/config.nix");
  config = if builtins.isFunction userConfig then userConfig { inherit lib; } else userConfig;
  maw = import (dotfiles + "/maw.nix");

  # every modules/<name>.nix, keyed by name
  moduleFiles = lib.filterAttrs (file: type: type == "regular" && lib.hasSuffix ".nix" file) (
    builtins.readDir (dotfiles + "/modules")
  );
  evalModule = file: lib.flatten (import (dotfiles + "/modules/${file}") { inherit config lib maw; });
in
{
  modules = lib.mapAttrs' (file: _: lib.nameValuePair (lib.removeSuffix ".nix" file) (evalModule file)) moduleFiles;

  # maw's own settings from config.nix, like maw.autoCommit
  settings = config.maw or { };
}
