# Usage

## Getting help

```sh
maw help              # list topics
maw help usage        # this guide; also modules, formats
maw help drift        # any section, by its heading
maw help add          # a command's options
```

Every command's `--help` ends with a `see:` line naming the section that explains it. On a terminal, help opens in `$PAGER` (`less` by default); set `NO_COLOR` to turn off styling.

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

Moving an existing setup in? Follow [migrating.md](migrating.md) instead.

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
- Files in `out/` that no module produces anymore are deleted.
- A file in `out/` edited by hand (usually through its link, see [drift](#drift)) is never overwritten. `maw build --force` backs up the edit and overwrites it.

## Activating

```sh
maw activate
```

Builds, then symlinks every file into place:

- rendered files link to `out/`: `~/.config/niri/config.kdl -> ~/dotfiles/out/niri/config.kdl`
- files in `static/<name>/` link as they are, each file on its own: `static/nvim/init.lua` goes to `~/.config/nvim/init.lua`

```
link ~/.config/niri/config.kdl
backup ~/.config/fuzzel/fuzzel.ini -> ~/.local/state/maw/backups/.config/fuzzel/fuzzel.ini
link ~/.config/fuzzel/fuzzel.ini
update ~/.config/waybar/style.css
unlink ~/.config/foot/foot.ini
```

- `link`: a new link. If a real file or someone else's link was already there, it's moved to the backups dir first (`backup`).
- `relink`: the file now comes from somewhere else, e.g. it moved from a module to `static/`.
- `update`: the content behind an existing link changed. Nothing needs doing on disk.
- `unlink`: a module or static file was removed, so its link is too. A backup made when it was first linked stays in the backups dir.

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

`maw activate --dry-run` prints the plan and changes nothing, not even `out/`.

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

Drifted files are left alone and reported on every run. To keep the change, move it into the module or `static/`. To discard it, run `maw activate --force`, which backs up the drifted file and puts maw's version back.

Editing a `static/` file through its link is not drift: the link points straight at `static/`, so you're editing the repo.

## Day to day

### Writing a module

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

### Editing

```sh
maw edit foot       # modules/foot.nix, or static/foot/ if there's no module
maw edit config     # config.nix
```

Opens `$VISUAL`, else `$EDITOR`, else `vi`, then activates. If the module doesn't evaluate, maw prints the error and asks whether to reopen it.

### Adding verbatim files

```sh
maw add ~/.config/nvim          # a whole dir, file by file
maw add ~/.bashrc
maw add ~/.tmux.conf tmux       # under an explicit name
```

Copies into `static/` and activates, so the original is backed up and replaced by a link to the copy. Each file lands back exactly where it came from:

- a path the registry knows is filed under that program: `~/.bashrc` becomes `static/bash/.bashrc`
- a path under `~/.config/<name>/` is filed under `<name>`: `static/nvim/init.lua`
- anything else goes at the top of `static/`, like a [loose file](#loose-static-files)

When the registry alone would put the copy somewhere else, the original path is recorded in `maw.nix` `paths`. Adding a file maw already links is an error.

`edit`, `new`, and `add` all take `--no-activate`, to change several things and activate once at the end, e.g. when moving an existing setup into maw.

### Checking

```sh
maw status
```

One line per file that isn't in sync, or `clean`:

| label | meaning |
|---|---|
| `missing` | a declared package isn't installed |
| `new` | declared, not linked yet |
| `blocked` | a file maw doesn't manage is in the way; activating backs it up |
| `changed` | a module changed and `out/` hasn't caught up |
| `moved` | the file now comes from somewhere else |
| `stale` | no longer declared; activating unlinks, deletes, or disables it |
| `replaced` | [drift](#drift): the link was replaced |
| `edited` | [drift](#drift): edited through the link |
| `unplaced` | a loose static file with no destination yet |
| `disabled` | a declared service isn't enabled |
| `restart` | a running service's files changed |

```sh
maw diff
```

A unified diff of each live file against what maw would put there, the same as `maw activate --force` would leave. Removed links diff against nothing. Neither command writes anything.

## Packages

Packages you want on the machine are declared in `maw.nix`, one list per source:

```nix
packages = {
  xbps = [ "foot" "niri" ];
  cargo = [ "bat" "typos-cli@1.24" "https://github.com/vitali87/croft.git" ];
  go = [ "github.com/jesseduffield/lazygit" ];
};
```

maw writes those lists; you change them with the commands below. `maw activate` installs anything declared but missing, before it links files. Nothing is ever removed just because it isn't declared.

### Installing

```sh
maw install foot                                   # from the void repos
maw install hello-cli                              # not in xbps: asks before using crates.io
maw install cargo:bat@0.24                         # a crate, pinned to a version
maw install cargo:https://github.com/user/tool.git # a crate from git
maw install go:github.com/jesseduffield/lazygit    # a go program
maw install foot --dry-run
```

```
install foot
record foot in maw.nix
create modules/foot.nix
```

A bare name goes to whichever source already declares it, else to xbps. If xbps doesn't have it but crates.io has a crate by that name, maw asks first:

```
hello-cli isn't in xbps; install crate hello-cli 0.2.2 from crates.io? [Y/n]
```

Without a terminal it stops and suggests `cargo:<name>` instead. Library crates, which build no program, are refused before any question, since there's nothing to install; add them to a project with `cargo add` instead. A `cargo:`, `go:`, or `xbps:` prefix skips all of that. `@version` pins a crate or go program to that version; without it you get the latest.

Then maw installs (`sudo xbps-install`, `cargo install --locked`, or `go install`), records the package in `maw.nix`, and, when the registry knows the program and the repo has no module or `static/` dir for it yet, creates its module the way `maw new` does, importing any config already on disk. Then it activates. A package that's already installed is only recorded. A name nothing has is an error.

cargo installs into `~/.cargo/bin`, go into `$GOBIN` (or `$GOPATH/bin`, or `~/go/bin`). maw leaves your shell config alone, but warns after an install if that dir isn't on your `PATH`, naming the line to add.

### Removing

```sh
maw remove foot
maw remove bat          # a crate, found by name
maw remove lazygit      # a go program, by its binary or path
```

Removes the package and drops it from `maw.nix`. xbps also removes dependencies nothing else needs; go programs are deleted from the bin dir. Its module stays, so reinstalling brings the config back; delete `modules/foot.nix` yourself if you're done with it. `--dry-run` prints the plan.

### Looking things up

```sh
maw query           # packages you installed or declared, in every source
maw query bat       # one of them
maw search term     # xbps, or crates.io when xbps has nothing; [*] marks installed
maw search cargo:term
maw info bat        # details, plus whether maw manages it
```

`query` lists what you installed by hand plus everything declared, flagging the two kinds of mismatch: `(undeclared)` for installed but not in `maw.nix`, `(missing)` for declared but not installed. Crates and go programs are marked with their source. `search` prints names the way `maw install` takes them. `info` shows the version, whether it's installed and declared, the module or `static/` dir holding its config, and where the registry puts that config.

### Updating

```sh
maw sync
```

Upgrades the system (`xbps-install -Su`), then every crate and go program that isn't pinned to a version. Pinned ones stay put until you install a different version.

## Services

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

### Service commands

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

### Logs

Services maw writes log through `svlogd`: system ones to `/var/log/<name>/`, user ones to `~/.local/state/log/<name>/`, rotated automatically. `maw sv log <name>` follows the `current` file.

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
```

`inputs` and `cache/` are safe to delete. Deleting `outputs` or `manifest` makes maw forget what it wrote, so drift goes unnoticed until the next activation.
