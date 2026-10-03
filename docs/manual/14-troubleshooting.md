# 14. Troubleshooting

When something on the machine doesn't work, start with `maw doctor`. For anything maw manages, the commands below show where it stands.

## Doctor

```sh
maw doctor
```

`status` compares your machine with your repo; `doctor` looks for problems that break a desktop quietly, which nothing in your repo would show:

```
warn no shared session bus
     declare a core dbus user service: `maw help modules`, under lib.service
warn /etc/pam.d/greetd doesn't unlock the keyring
     add `-auth optional pam_gnome_keyring.so` and `-session optional pam_gnome_keyring.so auto_start`
```

It checks for:

- a session bus the keyring, apps, and user services share
- greetd starting your session through `dbus-run-session`, which gives it a private bus
- greetd without a `vt`
- a keyring your login doesn't unlock
- Electron apps without the hint to use Wayland
- user services without `turnstiled`
- the bin dirs of declared cargo, go, uv, npm, and flatpak programs missing from `PATH`

With nothing to report, it prints `all good`.

## Where to look

- **A file isn't what you declared**: `maw status` lists everything out of sync, and `maw diff` shows each file's difference; a file edited in place is [drift](02-getting-started.md#drift).
- **A service isn't running**: `maw sv status <name>`, then `maw sv log <name>`; see [logs](08-services.md#logs).
- **A module doesn't evaluate**: `maw build` names the file and line; `maw edit <name>` reopens it after an error.
- **Before a risky change**: `maw activate --dry-run` lists every step without taking any.
- **After a bad change**: `maw rollback` returns files and packages to the last generation; see [rolling back](12-history.md#rolling-back).

Next: chapter 15, reference (`maw help 15`).
