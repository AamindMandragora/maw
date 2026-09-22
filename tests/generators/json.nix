let
  lib = import <maw/lib.nix>;
in
lib.toJSON {
  layer = "top";
  height = 30;
  modules-left = [ "niri/workspaces" ];
  empty = { };
  clock.format = "{:%H:%M}";
  custom = lib.raw ''{ "exec": "date" }'';
}
