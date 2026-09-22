# Usage

## Setting up

```sh
maw init ~/dotfiles
```

This creates the repo, or fills in whatever an existing one is missing, and records its path in `~/.config/maw/dotfiles`. Existing files are never overwritten.

```path
~/dotfiles/
  default.nix    # entry point, leave as is
  config.nix     # your shared values
  maw.nix        # written by maw
  modules/       # one <name>.nix per program
  static/        # verbatim files
  out/           # rendered output, committed with the repo
```

Every other command finds the repo through that recorded path, so it works from any directory.

## Building

```sh
maw build
```

Renders every module into `out/<name>/`, named after the file's destination: `out/niri/config.kdl`, `out/waybar/style.css`. Output lists what happened:

```
eval niri
write out/niri/config.kdl
remove out/foot/foot.ini
```

`up to date` means nothing needed doing.

Builds are incremental:

- A module is re-evaluated only when its file, `config.nix`, `maw.nix`, or maw itself changed. Editing `config.nix` re-evaluates every module.
- A file in `out/` is rewritten only when its content changed.
- Files in `out/` that no module produces anymore are deleted, so don't edit `out/` by hand.

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

maw keeps its own bookkeeping outside the repo, all safe to delete:

```path
~/.config/maw/dotfiles     # path of your repo
~/.local/state/maw/inputs  # stamps of input files, to skip re-reading unchanged ones
~/.local/state/maw/cache/  # evaluated modules, reused while their inputs are unchanged
```
