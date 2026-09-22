let
  lib = import <maw/lib.nix>;
in
lib.toINI {
  shell = "bash";
  main.term = "xterm-256color";
}
