let
  lib = import <maw/lib.nix>;
in
lib.toTOML {
  title = "greetd";
  ratio = 0.5;
  enabled = true;
  terminal.vt = 1;
  default_session = {
    command = "tuigreet --cmd niri-session";
    user = "greeter";
  };
  servers.alpha = {
    ip = "10.0.0.1";
    ports = [ 80 443 ];
  };
  bin = [
    { name = "maw"; path = "src/main.rs"; }
    { name = "tool"; }
  ];
  "key with space" = { inline = { a = 1; }; };
  extra = lib.raw ''"verbatim"'';
}
