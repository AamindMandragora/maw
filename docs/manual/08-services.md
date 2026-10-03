# 8. Services

Services are supervised by runit. There are two kinds:

- **system** services run from boot as root. Definitions live in `/etc/sv/<name>/`, enabled by a link in `/var/service/`.
- **user** services run as you, in your session, started by turnstile at login. Definitions live in `~/.config/sv/<name>/`, enabled by a link in `~/.config/service/`.

A service is enabled when `maw.nix` lists it, or when a module defines it with `lib.service` (see `maw help lib.service`):

```nix
services = {
  system = [ "NetworkManager" "dbus" ];
  user = [ "pipewire" ];
};
```

`maw activate` enables what's declared, disables what maw enabled that no longer is (`sv down` first), and restarts running services whose files changed:

```
copy /etc/sv/backup/run
enable backup
restart rclone (user)
```

Services maw didn't enable are never touched by activation.

## Service commands

```sh
maw sv list                   # declared and enabled services, and their state
maw sv enable NetworkManager  # record in maw.nix, then activate
maw sv disable bluetoothd     # stop and unlink now, drop from maw.nix
maw sv status greetd
maw sv restart greetd
maw sv log rclone             # follow its log
```

A name is looked up in maw.nix and modules first, then among enabled services, then among definitions; `--user` or `--system` picks one when a name exists in both, e.g. while moving a system service to your session. `enable` works for any service with a definition, like the ones Void packages install into `/etc/sv`. A service a module enables can't be disabled from here: set `enable = false` in its `lib.service`.

`sv list` flags `(disabled)` for declared but not enabled and `(undeclared)` for enabled but not declared. Reading a system service's state needs root, so it shows `?` unless `sudo` needs no password.

## Logs

Services maw writes log through `svlogd`: system ones to `/var/log/<name>/`, user ones to `~/.local/state/log/<name>/`, rotated automatically. `maw sv log <name>` follows the `current` file.

Next: chapter 9, secrets (`maw help 9`).
