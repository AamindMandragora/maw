let
  lib = import <maw/lib.nix>;
in
builtins.toJSON (
  lib.service "rclone" {
    scope = "user";
    env.RCLONE_CONFIG = "/home/me/.config/rclone/rclone.conf";
    run = "exec rclone mount drive: /home/me/drive";
  }
)
