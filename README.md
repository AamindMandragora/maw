# maw

A declarative system manager for [Void Linux](https://voidlinux.org). Your packages, runit services, and config files live in one git repo. You write config in Nix, maw renders it to each program's own format (ini, json, kdl, css, toml, ...) and puts it in place.

```sh
maw init ~/dotfiles
maw install foot                 # installs it, records it, and scaffolds modules/foot.nix
maw edit foot                    # edit the module; saving activates it
maw status                       # anything out of sync
maw rollback                     # back to the previous generation, package versions included
maw                              # the TUI
```

```nix
# modules/foot.nix
{ config, lib, ... }:
lib.program "foot" {
  format = "ini";
  settings.main.font = "${config.font.mono}:size=11";
}
```

Nix is only the config language: maw evaluates it and never builds from the Nix store. Every activation that changes something is a commit in your repo, so a new machine is `maw init <git-url>` away.

## Installing

maw is an xbps package built from [srcpkgs/maw/template](srcpkgs/maw/template). To run it from a checkout:

```sh
sudo xbps-install nix git
cargo build --release
./target/release/maw init ~/dotfiles
```

## Docs

The [manual](docs/README.md) is in fifteen chapters, from [getting started](docs/manual/02-getting-started.md) to [troubleshooting](docs/manual/14-troubleshooting.md). The same text is in `maw help` and the man pages. Contributors: [development](docs/development.md) covers layout, tests, and releasing.

## License

[MIT](LICENSE)
