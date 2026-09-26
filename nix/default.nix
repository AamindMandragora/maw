{ dotfiles }:
let
  lib = import ./lib.nix;

  # maw's own dirs, passed as search paths; missing when a module is rendered by hand
  found = lookup: if lookup.success then lookup.value else null;
  stateDir = found (builtins.tryEval <maw-state>);
  configDir = found (builtins.tryEval <maw-config>);

  # the wallpaper theme maw keeps in its state dir, or nothing before the first; its wallpaper is the absolute path
  themeFile = if stateDir == null then null else stateDir + "/theme.json";
  themeJson = if themeFile != null && builtins.pathExists themeFile then builtins.fromJSON (builtins.readFile themeFile) else null;
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

  # this machine's name, as maw was told it
  hostFile = if configDir == null then null else configDir + "/host";
  host = if hostFile != null && builtins.pathExists hostFile then lib.removeSuffix "\n" (builtins.readFile hostFile) else "";

  # a file of config values, or a function of lib, theme, and host
  load = file: let value = import file; in if builtins.isFunction value then call value { inherit lib theme host; } else value;

  # config.nix, with this machine's hosts/<name>.nix merged over it
  hostConfigFile = dotfiles + "/hosts/${host}.nix";
  config = lib.recursiveUpdate (load (dotfiles + "/config.nix")) (if builtins.pathExists hostConfigFile then load hostConfigFile else { });
  maw = import (dotfiles + "/maw.nix");

  # every modules/<name>.nix, keyed by name
  moduleFiles = lib.filterAttrs (file: type: type == "regular" && lib.hasSuffix ".nix" file) (
    builtins.readDir (dotfiles + "/modules")
  );
  # lib knows this machine for onHosts: a module's files on the named machines only
  hostLib = lib // { onHosts = names: value: if builtins.elem host names then value else [ ]; };
  evalModule = file: lib.flatten (call (import (dotfiles + "/modules/${file}")) { inherit config maw theme host; lib = hostLib; });
in
{
  modules = lib.mapAttrs' (file: _: lib.nameValuePair (lib.removeSuffix ".nix" file) (evalModule file)) moduleFiles;

  # maw's own settings from config.nix, like maw.autoCommit
  settings = config.maw or { };
}
