# 15. Reference

Where maw puts the files it renders, and the state it keeps outside your repo.

## Where files go

You never write destination paths. Each program name maps to destinations through the registry, checked in this order:

1. `paths` in your `maw.nix`, recorded when maw had to ask you
2. the registry shipped with maw (`/usr/share/maw/registry.nix`)
3. `~/.config/<name>/`, with the main file named `config`

A registry entry looks like this:

```nix
waybar = {
  format = "json";
  files = {
    main = "waybar/config.jsonc";
    style = "waybar/style.css";
  };
};
```

Paths in `files` are relative to `~/.config`, unless they start with `~/` (your home) or `/` (the system). File roles not listed in `files` go under `dir`, which defaults to the program name. `root = true` marks system files, and `executable = true` marks scripts.

## State

maw keeps its own bookkeeping outside the repo:

```path
~/.config/maw/dotfiles       # path of your repo
~/.local/state/maw/inputs    # stamps of input files, to skip re-reading unchanged ones
~/.local/state/maw/cache/    # evaluated modules, reused while their inputs are unchanged
~/.local/state/maw/outputs   # hash of each out/ file as maw last wrote it
~/.local/state/maw/manifest  # every link and root copy the last activation made, with content hashes, and the services it enabled
~/.local/state/maw/backups/  # files moved aside, mirrored by path under home (system/ for the rest)
~/.local/state/maw/generations  # one line per generation: number, time, commit, message, package versions
~/.local/state/maw/held      # xbps packages a rollback is holding back
~/.local/state/maw/patched   # packages built with patches, held so upgrades keep them
~/.local/state/maw/theme.json   # the wallpaper theme: image, chosen color, palette
~/.local/state/maw/secrets/  # decrypted copies of encrypted files, readable only by you
~/.local/state/maw/secrets.json # which encrypted files were decrypted, by content hash
~/.local/state/maw/themed/   # files that follow the wallpaper, as your programs read them
~/.local/state/maw/uv.json   # maw.uv from config.nix, for installing uv tools
~/.local/state/maw/wallpaper-colors.json  # how many candidate colors each wallpaper has, by content
~/.config/maw/host           # this machine's name
```

`inputs`, `cache/`, and `wallpaper-colors.json` are safe to delete. Deleting `outputs` or `manifest` makes maw forget what it wrote, so drift goes unnoticed until the next activation.
