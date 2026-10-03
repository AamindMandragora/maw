# 2. Getting started

A maw repo holds everything about your machine. This chapter makes one, renders it, and puts it in place: `init`, `build`, and `activate`, the three steps everything else builds on.

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
  out/           # rendered output, a dir per machine, committed with the repo
```

Every other command finds the repo through that recorded path, so it works from any directory.

Moving an existing setup in? Follow [migrating.md](13-migrating.md) instead.

### On a new machine

```sh
maw init https://github.com/you/dotfiles          # clones to ~/dotfiles
maw init git@github.com:you/dotfiles.git ~/dots   # or anywhere
```

Given a git url, `init` clones the repo, prints the plan of what activating would do, and asks `activate now? [Y/n]`. Saying yes installs every declared package, links and copies every file, and enables every service: the machine becomes the one the repo describes.

Nix isn't needed for that. If it isn't installed, maw activates from the committed `out/<machine>/` and its index, `out/<machine>/.maw/index.json`, which every activation keeps up to date; name the new machine after one that has been activated with nix (see [more than one machine](11-machines.md)). To do the same by hand: `maw activate --no-build`. Install nix (`maw install nix`) before editing modules; without it maw can't render them.

## Building

```sh
maw build
```

Renders every module into `out/<machine>/<name>/` (each machine renders into its own dir, since machines can differ), named after the file's destination: `out/laptop/niri/config.kdl`, `out/laptop/waybar/style.css`. Output lists what happened:

```
eval niri
write out/laptop/niri/config.kdl
remove out/laptop/foot/foot.ini
```

`up to date` means nothing needed doing.

Builds are incremental:

- A module is re-evaluated only when its file, `config.nix`, `maw.nix`, or maw itself changed. Editing `config.nix` re-evaluates every module.
- A file in `out/` is rewritten only when its content changed.
- Files in `out/` that no module produces anymore are deleted.
- A file in `out/` edited by hand (usually through its link, see [drift](#drift)) is never overwritten. `maw build --force` backs up the edit and overwrites it.

## Activating

```sh
maw activate
```

Builds, then symlinks every file into place:

- rendered files link to `out/`: `~/.config/niri/config.kdl -> ~/dotfiles/out/laptop/niri/config.kdl`
- files in `static/<name>/` link as they are, each file on its own: `static/nvim/init.lua` goes to `~/.config/nvim/init.lua`

```
link ~/.config/niri/config.kdl
backup ~/.config/fuzzel/fuzzel.ini -> ~/.local/state/maw/backups/.config/fuzzel/fuzzel.ini
link ~/.config/fuzzel/fuzzel.ini
update ~/.config/waybar/style.css
unlink ~/.config/foot/foot.ini
set /org/gnome/desktop/interface/color-scheme 'prefer-dark'
reload waybar
```

- `link`: a new link. If a real file or someone else's link was already there, it's moved to the backups dir first (`backup`).
- `relink`: the file now comes from somewhere else, e.g. it moved from a module to `static/`.
- `update`: the content behind an existing link changed. Nothing needs doing on disk.
- `unlink`: a module or static file was removed, so its link is too. A backup made when it was first linked stays in the backups dir.
- `set` / `reset`: a desktop setting declared with [`lib.dconf`](04-modules.md) was written, or reset once nothing declares it.
- `reload`: a program whose files changed was told to read them again, last, once everything is in place; see [modules.md](04-modules.md).

Since everything is a link, a change reaches the live file as soon as it's in `out/` or `static/`. Running `maw activate` twice in a row does nothing the second time.

### System files

Files under `/` (a registry entry with `root = true`, like greetd's `/etc/greetd/config.toml`) are copied with `sudo` instead of linked, since `/home` may not be mounted when the system reads them at boot:

```
backup /etc/greetd/config.toml -> ~/.local/state/maw/backups/system/etc/greetd/config.toml
copy /etc/greetd/config.toml
delete /etc/old/thing.conf
```

- `copy`: the file is new, or changed in the repo. The first time maw replaces a file it didn't write, the original is saved first (`backup`).
- `delete`: nothing declares it anymore. A file someone edited since maw copied it is left alone.

A copy edited by hand is [drift](#drift), like an edited link: reported as `edited`, left alone, and overwritten (after a backup) only with `--force`.

`maw activate --dry-run` prints the plan and changes nothing, not even `out/`. `maw activate --no-build` skips nix and activates what's committed in `out/`; see [on a new machine](#on-a-new-machine).

### Loose static files

Files at the top of `static/`, outside any `<name>/` dir, are placed by type:

| file | goes to |
|---|---|
| executable | `~/.local/bin/` |
| image (png, jpg, webp, ...) | `~/.local/share/wallpapers/` |
| font (ttf, otf, woff, ...) | `~/.local/share/fonts/` |

Anything else, or a file that fits more than one row, gets a question the first time you activate in a terminal:

```
static/notes.txt: [s]cript, [w]allpaper, [f]ont, or a path: ~/notes.txt
record notes.txt in maw.nix
```

A path works like a registry path: relative paths are under `~/.config`, and a path ending in `/` is a directory. The answer is recorded in `maw.nix` `paths`, so you're asked only once. An empty answer, or a run without a terminal, skips the file for now.

### Drift

maw never overwrites a change it didn't make. It reports two kinds:

```
drift ~/.config/niri/config.kdl: replaced by another file; --force to relink
drift ~/.config/waybar/style.css: edited in place; --force to overwrite
```

- **replaced**: the link is gone and something else sits there, often an app that saves by writing a new file.
- **edited**: the file was edited through its link, so `out/` holds your edit instead of what the module renders.

Desktop settings declared with [`lib.dconf`](04-modules.md) drift the same way: `drift /org/gnome/desktop/interface/color-scheme: changed since maw set it; --force to overwrite`.

Drifted files are left alone and reported on every run. To keep the change, move it into the module or `static/`. To discard it, run `maw activate --force`, which backs up the drifted file and puts maw's version back.

Editing a `static/` file through its link is not drift: the link points straight at `static/`, so you're editing the repo.

Next: chapter 3, day to day (`maw help 3`).
