let
  lib = import <maw/lib.nix>;
in
lib.toKeyValue {
  font = "mono";
  size = 11;
  enabled = true;
  include = [ "a.conf" "b.conf" ];
  command = lib.raw "exec foo --bar";
}
