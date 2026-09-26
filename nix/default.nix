{ dotfiles }:
let
  lib = import ./lib.nix;

  # the wallpaper theme maw keeps in its state dir, or nothing before the first; its wallpaper is the absolute path
  themeFile = <maw-state> + "/theme.json";
  themeJson = if builtins.pathExists themeFile then builtins.fromJSON (builtins.readFile themeFile) else null;
  theme =
    if themeJson == null then
      { }
    else
      {
        inherit (themeJson) wallpaper colors base16 source;
        inherit (themeJson.settings) mode;
      };

  # a function gets the arguments it names, so config.nix taking only { lib } keeps working; one naming none (args: ...) gets all
  call =
    function: available:
    let
      named = builtins.functionArgs function;
    in
    function (if named == { } then available else builtins.intersectAttrs named available);

  # config.nix may be a plain attrset or a function of lib and theme
  userConfig = import (dotfiles + "/config.nix");
  config = if builtins.isFunction userConfig then call userConfig { inherit lib theme; } else userConfig;
  maw = import (dotfiles + "/maw.nix");

  # every modules/<name>.nix, keyed by name
  moduleFiles = lib.filterAttrs (file: type: type == "regular" && lib.hasSuffix ".nix" file) (
    builtins.readDir (dotfiles + "/modules")
  );
  evalModule = file: lib.flatten (call (import (dotfiles + "/modules/${file}")) { inherit config lib maw theme; });
in
{
  modules = lib.mapAttrs' (file: _: lib.nameValuePair (lib.removeSuffix ".nix" file) (evalModule file)) moduleFiles;

  # maw's own settings from config.nix, like maw.autoCommit
  settings = config.maw or { };
}
