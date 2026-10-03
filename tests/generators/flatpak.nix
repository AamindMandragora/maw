let
  lib = import <maw/lib.nix>;
in
(builtins.head (
  lib.flatpak "com.slack.Slack" {
    env.ELECTRON_OZONE_PLATFORM_HINT = "auto";
    sockets = [ "wayland" "!x11" ];
    filesystems = [ "~/Documents" ];
    talk = [ "org.freedesktop.Notifications" ];
  }
)).content
