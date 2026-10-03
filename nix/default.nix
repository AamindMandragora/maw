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

  # a function gets only the arguments it names; one naming none (args: ...) gets all
  call =
    function: available:
    let
      named = builtins.functionArgs function;
    in
    function (if named == { } then available else builtins.intersectAttrs named available);

  # this machine's name, as maw was told it
  hostFile = if configDir == null then null else configDir + "/host";
  host = if hostFile != null && builtins.pathExists hostFile then lib.removeSuffix "\n" (builtins.readFile hostFile) else "";

  # lib knows this machine for onHosts: a module's files on the named machines only
  hostLib = lib // { onHosts = names: value: if builtins.elem host names then value else [ ]; };

  # the theme with every color a fixed gray and the wallpaper a placeholder: what out/ is rendered with, so the files
  # git tracks stay the same whatever the wallpaper
  gray = value: if lib.hasPrefix "#" value then "#808080" else "808080";
  placeholder =
    value:
    if builtins.isAttrs value then
      builtins.mapAttrs (_: placeholder) value
    else if builtins.isList value then
      map placeholder value
    else if builtins.isString value && builtins.match "#?[0-9a-fA-F]{6}" value != null then
      gray value
    else
      value;
  plainTheme = if theme == { } then { } else placeholder theme // { wallpaper = "<wallpaper>"; };

  # a file of config values, or a function of lib, theme, and host
  load = theme: file: let value = import file; in if builtins.isFunction value then call value { inherit theme host; lib = hostLib; } else value;

  # config.nix, with this machine's hosts/<name>.nix merged over it; attrsets merge, lists and other values are replaced
  hostConfigFile = dotfiles + "/hosts/${host}.nix";
  configWith = theme: lib.recursiveUpdate (load theme (dotfiles + "/config.nix")) (if builtins.pathExists hostConfigFile then load theme hostConfigFile else { });
  config = configWith theme;
  maw = import (dotfiles + "/maw.nix");

  # every modules/<name>.nix, keyed by name
  moduleFiles = lib.filterAttrs (file: type: type == "regular" && lib.hasSuffix ".nix" file) (
    builtins.readDir (dotfiles + "/modules")
  );
  evalWith = theme: file: lib.flatten (call (import (dotfiles + "/modules/${file}")) { inherit maw theme host; config = configWith theme; lib = hostLib; });

  # each file with its placeholder rendering beside it, when there's a theme and both renderings have the same files
  evalModule =
    file:
    let
      files = evalWith theme file;
      plain = evalWith plainTheme file;
      same = builtins.length plain == builtins.length files && builtins.all (pair: pair.fst.name == pair.snd.name && pair.fst.key == pair.snd.key) (lib.zipLists files plain);
    in
    if theme == { } || !same then files else lib.zipListsWith (file: plain: file // { inherit plain; }) files plain;
in
{
  modules = lib.mapAttrs' (file: _: lib.nameValuePair (lib.removeSuffix ".nix" file) (evalModule file)) moduleFiles;

  # maw's own settings from config.nix, like maw.autoCommit
  settings = config.maw or { };
}
