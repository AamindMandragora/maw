# 3. Day to day

Most of the time with maw is changing config: a new module for a program, an edit, a file copied in as it is, and a look at what's out of sync before activating.

## Writing a module

```sh
maw new foot
maw new foot --format ini
```

Creates `modules/foot.nix`, opens it in your editor, and activates when you close it. The format defaults to the registry's for that program (`raw` if it has none), and a program with several files gets a `files` set with a format per file, guessed from each file's extension.

If the program already has a config where the module will put it, the file is imported as `lib.raw`, so the module renders exactly what you had:

```nix
{ config, lib, ... }:
lib.program "foot" {
  format = "ini";
  # imported from ~/.config/foot/foot.ini
  settings = lib.raw ''
    [main]
    font=monospace:size=11
  '';
}
```

From there, move settings out of the raw block into Nix at your own pace.

## Editing

```sh
maw edit foot       # modules/foot.nix, or static/foot/ if there's no module
maw edit config     # config.nix
```

Opens `$VISUAL`, else `$EDITOR`, else `vi`, then activates. If the module doesn't evaluate, maw prints the error and asks whether to reopen it.

## Adding verbatim files

```sh
maw add ~/.config/nvim          # a whole dir, file by file
maw add ~/.bashrc
maw add ~/.tmux.conf tmux       # under an explicit name
```

Copies into `static/` and activates, so the original is backed up and replaced by a link to the copy. Each file lands back exactly where it came from:

- a path the registry knows is filed under that program: `~/.bashrc` becomes `static/bash/.bashrc`
- a path under `~/.config/<name>/` is filed under `<name>`: `static/nvim/init.lua`
- anything else goes at the top of `static/`, like a [loose file](02-getting-started.md#loose-static-files)

When the registry alone would put the copy somewhere else, the original path is recorded in `maw.nix` `paths`. Adding a file maw already links is an error.

`edit`, `new`, and `add` all take `--no-activate`, to change several things and activate once at the end, e.g. when moving an existing setup into maw.

## Checking

```sh
maw status
```

The one report of everything out of sync, one line each, or `clean`. First what `maw activate` would change, then what `maw.nix` doesn't cover:

| label | meaning |
|---|---|
| `missing` | a declared package isn't installed |
| `new` | declared, not linked yet |
| `blocked` | a file maw doesn't manage is in the way; activating backs it up |
| `changed` | a module changed and `out/` hasn't caught up, or a declared desktop setting isn't set yet |
| `moved` | the file now comes from somewhere else |
| `stale` | no longer declared; activating unlinks, deletes, or disables it |
| `replaced` | [drift](02-getting-started.md#drift): the link was replaced |
| `edited` | [drift](02-getting-started.md#drift): edited through the link, or a desktop setting changed by hand |
| `skipped` | desktop settings can't be applied now, e.g. outside a desktop session |
| `unplaced` | a loose static file with no destination yet |
| `disabled` | a declared service isn't enabled |
| `restart` | a running service's files changed |
| `undeclared` | installed by hand or enabled, but not in `maw.nix`; see [adopting](06-packages.md#adopting) |
| `orphan` | a module or `static/` dir for a program that isn't installed |
| `outdated` | a source package whose template is at a newer version than what's installed, or whose patches aren't built into the installed package yet; see [source packages](07-source-packages.md) |

A program counts as installed when any source has a package by that name or a command by that name is on your `PATH`, so `static/nvim/` isn't an orphan while `neovim` is installed. Data dirs like `fonts` and `wallpapers` are never orphans.

```sh
maw diff
```

A unified diff of each live file against what maw would put there, the same as `maw activate --force` would leave. Removed links diff against nothing. Neither command writes anything.

Next: chapter 4, modules (`maw help 4`).
