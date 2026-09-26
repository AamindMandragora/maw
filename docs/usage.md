# Usage

## Getting help

```sh
maw help              # list topics
maw help usage        # this guide; also modules, formats
maw help drift        # any section, by its heading
maw help add          # a command's options
```

Every command's `--help` ends with a `see:` line naming the section that explains it. On a terminal, help opens in `$PAGER` (`less` by default); set `NO_COLOR` to turn off styling.

## The TUI

```sh
maw
```

With no arguments, maw opens a full-screen view of everything it manages, one tab per area:

| tab | shows | keys |
|---|---|---|
| 1 packages | what's installed and declared, per source | `i` install, `r` remove, `f` find in the repos, `s` sync, `A` adopt |
| 2 modules | modules and static dirs, with a preview of the rendered file | `e`/enter edit, `n` new, `a` add a file |
| 3 services | declared and enabled services, with the selected one's log | `e` enable, `d` disable, `r` restart, `s` status |
| 4 status | the same report as `maw status` | `a` activate, `F` activate --force |
| 5 diff | the same diff as `maw diff` | `a` activate |
| 6 history | generations, newest first | `r`/enter roll back to the selected one |
| 7 git | the repo's status and recent commits | `c` commit, `p` push, `P` pull |

Everywhere: `j`/`k` or up/down move, `g`/`G` jump to the top or bottom, `h`/`l`, left/right, `1`-`7`, or tab switch tabs, `/` filters the list, `o` shows or hides the output pane, `R` reloads, `?` lists the keys, `q` quits. The mouse works too: click a tab or a row, scroll to move. On a narrow window the tab names shorten and previews move below the list.

Every action runs exactly what the matching command runs. Its output streams into a pane at the bottom, questions (a commit message, `[Y/n]`) appear as popups, and your editor takes over the screen until you close it. Removing, disabling, rolling back, and forcing ask for confirmation first. It asks for your sudo password when it opens, and again before an action if that has expired.

## Completions and man pages

The maw package installs tab completion for bash, zsh, and fish, and man pages: `man maw` for every command (`man maw-install`, `man maw-sv-enable`, ...), and the guides as `maw-usage(7)`, `maw-migrating(7)`, `maw-modules(5)`, and `maw-formats(5)`, the same text as `maw help`.

Completion knows your own names, not just commands: `maw edit <tab>` offers your modules, `maw remove <tab>` your declared packages, `maw sv restart <tab>` your services, `maw rollback <tab>` your generations with their messages. Running maw from a source checkout, load it yourself:

```sh
source <(COMPLETE=bash maw)                 # bash, in ~/.bashrc
source <(COMPLETE=zsh maw)                  # zsh, in ~/.zshrc
COMPLETE=fish maw | source                  # fish, in config.fish
```

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

Moving an existing setup in? Follow [migrating.md](migrating.md) instead.

### On a new machine

```sh
maw init https://github.com/you/dotfiles          # clones to ~/dotfiles
maw init git@github.com:you/dotfiles.git ~/dots   # or anywhere
```

Given a git url, `init` clones the repo, prints the plan of what activating would do, and asks `activate now? [Y/n]`. Saying yes installs every declared package, links and copies every file, and enables every service: the machine becomes the one the repo describes.

Nix isn't needed for that. If it isn't installed, maw activates from the committed `out/<machine>/` and its index, `out/<machine>/.maw/index.json`, which every activation keeps up to date; name the new machine after one that has been activated with nix (see [more than one machine](#more-than-one-machine)). To do the same by hand: `maw activate --no-build`. Install nix (`maw install nix`) before editing modules; without it maw can't render them.

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
- `set` / `reset`: a desktop setting declared with [`lib.dconf`](modules.md) was written, or reset once nothing declares it.
- `reload`: a program whose files changed was told to read them again, last, once everything is in place; see [modules.md](modules.md).

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

Desktop settings declared with [`lib.dconf`](modules.md) drift the same way: `drift /org/gnome/desktop/interface/color-scheme: changed since maw set it; --force to overwrite`.

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

### Encrypted files

Files with passwords or tokens, like `rclone.conf`, can live in the repo encrypted, so it can be public:

```sh
maw add --secret ~/.config/rclone/rclone.conf   # encrypted into static/rclone/rclone.conf.age
maw secret edit ~/.config/rclone/rclone.conf    # decrypted into your editor, encrypted back
maw secret rekey                                # re-encrypted for every machine, after adding one
```

```
record hosts/laptop.pub
encrypt ~/.config/rclone/rclone.conf -> static/rclone/rclone.conf.age
```

Files are encrypted with [age](https://age-encryption.org) to your SSH key: `~/.ssh/id_ed25519`, else `~/.ssh/id_rsa`, or `maw.secretKey = "~/.ssh/other";` in `config.nix`. Each machine's public key is in the repo as `hosts/<name>.pub` (written the first time a machine encrypts), and every file is encrypted to all of them, so each machine decrypts with its own key and no private key leaves its machine. It needs `age` (`maw install age`); maw itself doesn't depend on it.

On activation, an encrypted file is decrypted into `~/.local/state/maw/secrets/` (readable only by you) and the live file links to that copy; the plaintext never enters the repo. It's decrypted again only when the encrypted file changes, so a key with a passphrase is asked for rarely. Edits made through the live file last until the encrypted file next changes; `maw secret edit` is how an edit reaches the repo.

A new machine can't decrypt anything until it's a recipient. Its first activation skips encrypted files with a note (`skip static/rclone/rclone.conf.age: ... isn't encrypted for this machine`). To add it: run `maw secret rekey` there, which records its key as `hosts/<name>.pub` (and warns about each file it can't open yet), push; then `maw secret rekey` and push on a machine that can decrypt, and pull on the new one.

### Checking

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
| `replaced` | [drift](#drift): the link was replaced |
| `edited` | [drift](#drift): edited through the link, or a desktop setting changed by hand |
| `skipped` | desktop settings can't be applied now, e.g. outside a desktop session |
| `unplaced` | a loose static file with no destination yet |
| `disabled` | a declared service isn't enabled |
| `restart` | a running service's files changed |
| `undeclared` | installed by hand or enabled, but not in `maw.nix`; see [adopting](#adopting) |
| `orphan` | a module or `static/` dir for a program that isn't installed |
| `outdated` | a source package whose template is at a newer version than what's installed, or whose patches aren't built into the installed package yet; see [source packages](#source-packages) |

A program counts as installed when any source has a package by that name or a command by that name is on your `PATH`, so `static/nvim/` is fine with the `neovim` package. Data dirs like `fonts` and `wallpapers` are never orphans.

```sh
maw diff
```

A unified diff of each live file against what maw would put there, the same as `maw activate --force` would leave. Removed links diff against nothing. Neither command writes anything.

### More than one machine

One repo can serve several machines that are mostly alike, like a laptop and a desktop. Each machine has a name: `maw init` asks for it (your hostname by default), and `maw host` shows it, or renames the machine and activates for what the new name gets (not a generation, since the name belongs to the machine):

```sh
maw host            # laptop, with hosts/laptop.nix
maw host desktop    # this machine is now desktop
```

The name is kept in `~/.config/maw/host`, and each machine renders into `out/<name>/`. (A repo from before machines had names keeps its rendered files straight under `out/`; the first activation moves them into this machine's dir.) What differs between machines goes in three places:

- **Config values**: `hosts/<name>.nix` is merged over `config.nix` on that machine only, key by key, so it holds just what differs:

  ```nix
  # hosts/laptop.nix
  { output = "eDP-1"; scale = 2; font.size = 13; }
  ```

  It can be a function of `{ lib, theme, host }` like `config.nix`.
- **Modules**: every module gets `host`, the machine's name, to decide with. `lib.onHosts` limits a whole module to some machines, so the others don't get its files or services:

  ```nix
  { lib, ... }:
  lib.onHosts [ "laptop" ] (lib.service "tlp" { run = "exec tlp start"; })
  ```

- **Packages and services**: `maw install --here tlp` and `maw sv enable --here tlp` record them for this machine only, under `hosts` in `maw.nix`; `maw adopt` takes `here <name>` as well as `keep`. `maw remove` and `maw sv disable` drop a name from both the shared lists and this machine's.

  ```nix
  hosts = {
    laptop = {
      packages = {
        xbps = [ "tlp" ];
      };
      services = {
        system = [ "tlp" ];
      };
    };
  };
  ```

Everything else (`status`, `query`, `adopt`, rollback) sees what this machine declares: the shared lists plus its own. A second machine gets set up with `maw init <your repo url>`: it asks the name first, then activates what that name declares.

### Wallpaper themes

maw can make a color palette from your wallpaper with [matugen](https://github.com/InioX/matugen), for every module to use. It's optional: nothing about it runs, and matugen isn't needed, until you set `maw.theme` in `config.nix` or run `maw wallpaper`. Wallpapers live in `static/wallpapers/`, so every machine has them:

```sh
maw wallpaper ~/Pictures/forest.jpg   # copied into static/wallpapers/, then themed
maw wallpaper random                  # another wallpaper from static/wallpapers/
maw wallpaper                         # which one it is now
```

```
copy static/wallpapers/forest.jpg
theme static/wallpapers/forest.jpg: color 2 of 4, dark #a6d0b0
eval niri waybar fuzzel
update ~/.config/waybar/style.css
reload waybar
```

An image has a few candidate colors a palette can grow from, most dominant first; each `maw wallpaper` picks one at random, so the same wallpaper can come back in a different color. Then maw activates, so modules that use the theme rebuild and their programs reload. Changing the wallpaper isn't a generation: the current theme is this machine's state, not history, and themed files in `out/` are committed with your next real change. `pull` and `rollback` rebuild `out/` anyway, so those changes never get in their way.

In `config.nix`, the palette's settings, with their defaults (setting `maw.theme` also themes a fresh machine from the first wallpaper):

```nix
maw.theme = {
  mode = "dark";                  # or "light"
  scheme = "scheme-tonal-spot";   # or scheme-vibrant, scheme-expressive, scheme-fidelity, scheme-content, ...
  contrast = 0;                   # -1 to 1
};
```

Modules and `config.nix` read it as `theme` (see [modules.md](modules.md)):

- `theme.colors`: Material You colors as `#rrggbb`: `primary`, `on_primary`, `surface`, `on_surface`, `surface_container`, `outline`, `error`, `secondary`, `tertiary`, and the rest of the scheme
- `theme.base16`: `base00` through `base0F`, for programs with base16 themes
- `theme.wallpaper`: the image's absolute path
- `theme.mode`, `theme.source`: the mode and the color the palette grew from

To show the wallpaper, declare it like anything else. A service is best: the path is in its run file, so changing the wallpaper restarts it:

```nix
{ lib, theme, ... }:
lib.service "swaybg" {
  scope = "user";
  run = "exec swaybg -i ${theme.wallpaper} -m fill";
}
```

To change it on a timer, run `maw wallpaper random` from anything that runs on a schedule, like a user service that sleeps between changes. Theming needs `matugen` (`maw install matugen`); maw itself doesn't depend on it.

### Adopting

```sh
maw adopt
maw adopt --dry-run    # print the checklist instead
```

Lists everything `status` calls `undeclared` in your editor, one line each, every line starting as `keep`:

```
# maw adopt: `keep` records it in maw.nix, `skip` (or deleting the line) ignores it from now on

# packages (xbps)
keep firefox
skip base-devel

# packages (cargo)
keep https://github.com/vitali87/croft.git

# services (system)
keep NetworkManager
skip agetty-tty3
```

Save and quit, and `keep` lines are recorded in `maw.nix` like `maw install` or `maw sv enable` would, while `skip` lines go under `ignored`:

```nix
ignored = {
  packages = {
    xbps = [ "base-devel" ];
  };
  services = {
    system = [ "agetty-tty3" ];
  };
};
```

Ignored things stay installed and enabled; maw just stops mentioning them. Nothing is installed, removed, or restarted by `adopt`. Only packages you installed by hand are offered, never their dependencies.

## Packages

Packages you want on the machine are declared in `maw.nix`, one list per source:

```nix
packages = {
  xbps = [ "foot" "niri" ];
  flatpak = [ "com.slack.Slack" "com.tomjwatson.Emote" ];
  cargo = [ "bat" "typos-cli@1.24" "https://github.com/vitali87/croft.git" ];
  go = [ "github.com/jesseduffield/lazygit" ];
  uv = [ "ruff" "httpie@3.2.3" ];
  npm = [ "prettier" ];
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
maw install com.slack.Slack                        # a flatpak app, by its app id
maw install uv:ruff                                # a python program from pypi
maw install uv:git+https://github.com/user/tool    # a python program from git
maw install npm:prettier                           # a javascript program from npm
maw install foot --dry-run
```

```
install foot
record foot in maw.nix
create modules/foot.nix
```

A bare name goes to whichever source already declares it, a flatpak app id (`com.slack.Slack`) to Flathub, and anything else to xbps. If xbps doesn't have it, maw looks for that exact name on Flathub, crates.io, PyPI, and npm, and asks first; with more than one match, it asks which:

```
prettier isn't in xbps; install one of:
  1  npm:prettier 3.6.2  Prettier is an opinionated code formatter
  2  cargo:prettier 0.1.0  ...
which? [1]
```

Without a terminal it stops and names the prefixes to use instead. When no source has the name, maw looks in the AUR and nixpkgs and offers to [draft a template](#source-packages) from one; `maw install aur:<name>` or `nixpkgs:<name>` goes straight there. The draft opens in your editor, and `maw install <name>` then builds and installs it. Library crates and npm packages with no commands are refused before any question, since there's nothing to install; add them to a project with `cargo add` or `npm install` instead. A `flatpak:`, `cargo:`, `go:`, `uv:`, `npm:`, or `xbps:` prefix skips all of that. Python and npm packages always need their prefix. `@version` pins a crate, go program, python program, or npm package to that version; without it you get the latest.

Each source needs its tool: `cargo`, `go`, `uv`, `flatpak`, or `npm` (Void's `nodejs`). When one is missing, maw stops and says what to install: `error: uv isn't installed; maw install uv first`. `maw install uv uv:ruff` does both, xbps first.

Then maw installs (`sudo xbps-install`, `sudo flatpak install`, `cargo install --locked`, `go install`, `uv tool install`, or `npm install -g`), records the package in `maw.nix`, and, when the registry knows the program and the repo has no module or `static/` dir for it yet, creates its module the way `maw new` does, importing any config already on disk. Then it activates. A package that's already installed is only recorded. A name nothing has is an error.

cargo installs into `~/.cargo/bin`, go into `$GOBIN` (or `$GOPATH/bin`, or `~/go/bin`), and uv and npm into `~/.local/bin` (npm with `~/.local` as its global prefix, so nothing needs root). maw leaves your shell config alone, but warns after an install if that dir isn't on your `PATH`, naming the line to add.

#### Flatpak apps

Flatpaks come from Flathub (maw adds the remote if it's missing) and are installed system-wide, for every user. Each declared app also gets a wrapper in `~/.local/bin` named after the last part of its id, so configs and shells can run it by that name: `emote` runs `flatpak run com.tomjwatson.Emote`. To pick another name, in `config.nix`:

```nix
maw.flatpakNames = { "us.zoom.Zoom" = "zoom-meet"; };
```

App launchers like fuzzel find flatpaks through their desktop entries either way. `@<commit>` pins an app to an exact build, which is how generations record them.

### Removing

```sh
maw remove foot
maw remove bat          # a crate, found by name
maw remove lazygit      # a go program, by its binary or path
maw remove emote        # a flatpak, by its id or wrapper name
```

Removes the package and drops it from `maw.nix`. xbps also removes dependencies nothing else needs, and flatpak runtimes nothing uses any more; go programs are deleted from the bin dir. Its module stays, so reinstalling brings the config back; delete `modules/foot.nix` yourself if you're done with it. `--dry-run` prints the plan.

### Looking things up

```sh
maw query           # packages you installed or declared, in every source
maw query bat       # one of them
maw search term     # every source; [*] installed, [~] draftable
maw search npm:term # one source: xbps, flatpak, cargo, uv, npm, aur, or nixpkgs
maw info bat        # details, plus whether maw manages it
```

`query` lists what you installed by hand plus everything declared, flagging the two kinds of mismatch: `(undeclared)` for installed but not in `maw.nix`, `(missing)` for declared but not installed. Crates and go programs are marked with their source. `search` looks in xbps, Flathub, crates.io, PyPI (exact names only, since PyPI has no search), and npm, and prints names the way `maw install` takes them. Only when none of them has a match does it look in the AUR and nixpkgs, whose results are marked `[~]`: they can't be installed as they are, only drafted into a template. A source whose tool isn't installed is skipped; one that doesn't answer gets a warning. nixpkgs is searched through [search.nixos.org](https://search.nixos.org). `info` shows the version, whether it's installed and declared, the module or `static/` dir holding its config, and where the registry puts that config.

### Source packages

Anything the Void repos don't have, you can build yourself with an xbps-src template in `srcpkgs/<name>/template`:

```sh
maw src new hello        # writes a blank template and opens it
maw install hello        # builds it with xbps-src, installs it, records it
maw src build hello      # rebuilds after you change the template
```

To draft a template from nixpkgs instead of writing one:

```sh
maw src new lazygit --from-nix           # from nixpkgs' lazygit
maw src new rg --from-nix ripgrep        # a different name than nixpkgs uses
maw src update lazygit                   # later: move it to nixpkgs' current version, then rebuild
```

maw reads the package's metadata from nixpkgs (it never builds with nix) and fills in the version, description, license, homepage, source url and its checksum, the build style (go, cargo, meson, cmake, python, or configure), and the dependencies, using Void's names:

```
# scaffolded by maw from nixpkgs 'ripgrep' at 4975466d3247
pkgname=ripgrep
version=15.2.0
revision=1
build_style=cargo
hostmakedepends="pkg-config"
makedepends="pcre2-devel"
# TODO: nix had 'libfoo'
...
```

A dependency maw can't match to a Void package is left as a `# TODO` line. Fix it by hand; when you close the editor, maw asks whether to remember what you replaced it with (`record libfoo -> foo-devel in depmap? [Y/n]`) and saves it to `depmap.nix` in your repo, so the next scaffold gets it right. Nix-only build helpers are dropped on their own. Check a draft before building: nixpkgs sometimes patches or configures a package in ways a template needs spelled out, like ripgrep's `configure_args="--features=pcre2"`.

The nixpkgs checkout lives at `~/.local/share/maw/nixpkgs` (a shallow nixos-unstable clone, about 400MB), made on first use; `maw.nixpkgs` in `config.nix` points elsewhere.

Or from an AUR package, which works the same way:

```sh
maw src new wlogout --from-aur           # from the aur's wlogout
maw src new yay --from-aur yay-bin       # a different name than the aur uses
```

maw reads the AUR's metadata and the package's PKGBUILD. It never runs the PKGBUILD: it reads its plain assignments and simple `${var}` expansions, so anything cleverer shows up as a `# TODO`. The build style comes from the build tools and the PKGBUILD's `build()`. Arch's `depends` holds both libraries and programs, so libraries go to `makedepends` as their `-devel` package and programs to `depends`. The PKGBUILD's `package()` is kept as comments at the end of the draft, for install steps the build style doesn't cover (licenses, completions, config files):

```
# the PKGBUILD's package(), for anything the build style doesn't cover:
#	install -Dm644 man/paru.8 "$pkgdir/usr/share/man/man8/paru.8"
#	install -Dm644 completions/zsh "${pkgdir}/usr/share/zsh/site-functions/_paru"
```

A `-bin` package that repackages prebuilt files gets no build style and a TODO to port its `package()` into `do_install()`. A `-git` package builds from a checkout, which xbps-src can't do; draft from the release package instead. Arch-only dependencies, like `pacman`, stay as TODOs to delete.

`src update` works on templates drafted either way, which it recognizes by their `# scaffolded by maw` line; it touches only `version`, `revision`, `distfiles`, and `checksum`, so your other edits stay. On a template you wrote yourself, it follows the upstream's releases instead; see [updating](#updating).

#### Patching Void's packages

To change a package Void already has, give it patches instead of a template:

```
srcpkgs/gnome-network-displays/
  patches/
    gnd-dmabuf-gl.patch
```

```sh
maw src build gnome-network-displays    # void's template, plus your patches
```

maw builds Void's own template with your patches applied after Void's (in name order, or the order of a `series` file you put next to them), installs it in place of Void's build, and holds it so `maw sync` never swaps Void's unpatched build back. When Void releases a new version, `maw status` shows `outdated gnome-network-displays 0.99.0_1 -> 1.0.0_1 + patches`, and `maw src build` (or the next `maw sync`) updates the clone to Void's latest templates and rebuilds with your patches. If a patch stops applying to the new version, the build fails and the patched old version stays installed. Delete the `srcpkgs/` dir to go back to Void's build; the next `maw sync` releases the hold and reinstalls it.

A name with a template in `srcpkgs/` is always a source build, and it's recorded under `packages.xbps` like any other package; the template's presence is what makes it one. The template is written in xbps-src's own format; see the [Void manual on templates](https://github.com/void-linux/void-packages/blob/master/Manual.md).

Builds happen in a void-packages clone maw keeps at `~/.local/share/maw/void-packages`. The first build clones it (shallowly) and bootstraps its build root, which takes a few minutes; after that, each template is copied into the clone's `srcpkgs/` and built with `xbps-src pkg <name>`, and the package is installed from the clone's `hostdir/binpkgs`. A template can't use the name of a package void-packages already has; patch that package instead. To build in a clone of your own instead, in `config.nix`:

```nix
maw.voidPackages = "~/void-packages";
```

Activation builds and installs a declared source package that isn't installed at all. It doesn't rebuild on its own when you change a template: `maw status` shows `outdated hello 0.1_1 -> 0.2_1`, and `maw src build hello` (or the next `maw sync`) rebuilds it and upgrades the installed package.

While any source package is declared, maw also manages `/etc/xbps.d/10-maw-local.conf`, which adds the clone's `hostdir/binpkgs` to xbps's repositories, so `xbps-query`, `xbps-install`, and `xbps-install -Su` see your builds like any other package.

### Updating

```sh
maw sync
```

Upgrades the system (`xbps-install -Su`), then every flatpak, crate, go, python, and npm program that isn't pinned to a version. Then it moves each [source package](#source-packages) template that follows releases to its upstream's newest, and rebuilds every source package whose template is ahead of what's installed. That's how maw updates itself: its template follows maw's releases.

A template follows releases when it downloads a tagged release from GitHub, GitLab, Codeberg, or sourcehut with `${version}` in the url, like `distfiles="${homepage}/archive/refs/tags/v${version}.tar.gz"`. maw reads the repo's tags (`git ls-remote`), takes the newest plain version matching the url's tag (`v${version}` finds `v0.3.0`, skipping pre-releases like `v0.3.0-rc1`), and rewrites `version`, `revision`, and `checksum`. `maw src update <name>` does the same for one template. A `# maw: hold` line keeps a template where it is. Pinned ones stay put until you install a different version, and so do xbps packages a [rollback](#rolling-back) held back. `maw sync --release` releases those first.

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

## Generations

Every activation that changes something is a generation: maw commits your whole repo (modules, static files, `config.nix`, `maw.nix`, and `out/`) and records it. The same happens after `install`, `remove`, `sv enable`, `sv disable`, `adopt`, and `pull`, since each ends by activating.

Before committing, maw asks for a message, offering a generated one:

```
commit message [generation 12: niri, waybar; 2 links; install foot]:
generation 12 (3f9a2c1)
```

Press enter to take it. Without a terminal the generated one is used.

```sh
maw generations
```

```
  11  2026-09-22 21:04  e761146  initial dotfiles
  12  2026-09-23 14:02  3f9a2c1  generation 12: niri, waybar; 2 links; install foot
```

Each generation also records the exact version of every installed package in every source, in `~/.local/state/maw/generations`. That log belongs to the machine, not the repo: two machines sharing a repo each have their own.

To skip committing once, `maw activate --no-commit`. To turn it off, in `config.nix`:

```nix
maw.autoCommit = false;
```

To commit by hand, e.g. edits you haven't activated yet:

```sh
maw commit -m "try a lighter bar"
maw commit            # git opens your editor for the message
```

### Rolling back

```sh
maw rollback            # to the generation before the latest
maw rollback 11         # to generation 11
maw rollback --dry-run
```

```
restore generation 11 (e761146)
remove cargo:bat
install libportal-0.10.0_1
keep firefox: firefox-150.0_1 isn't in the cache or the repo
hold libportal
```

A rollback is itself a new generation, so history only moves forward and you can roll back a rollback. It:

1. restores the repo to that generation's commit (it needs no uncommitted changes, so `maw commit` first),
2. removes packages declared now but not then, and puts every package declared then back at the version that generation recorded,
3. activates, which relinks config and re-enables services the way they were.

Only declared packages change; ones you installed by hand and never adopted are left alone. An old xbps version comes from xbps's download cache in `/var/cache/xbps` (which xbps keeps unless you clean it), from an earlier build of your own in the void-packages clone's `hostdir/binpkgs`, or from the repo if it's still current there. A version found in neither stays as it is, and the plan says so. Flatpaks go back to their recorded commit, and everything else is reinstalled at its recorded version.

xbps packages left behind the repo's newest version are held, so `maw sync` doesn't undo the rollback. They stay held until `maw sync --release`, or until a later rollback puts them back at the newest version.

### Sharing

```sh
maw push      # to the repo's remote, setting it as upstream
maw pull      # fast-forward only, then activate
```

Both need a remote: `git -C ~/dotfiles remote add origin <url>`. `pull` refuses to merge; if both sides changed, sort it out with git, then `maw activate`.

Each machine renders into its own `out/<machine>/`, so machines never rewrite each other's output: a pull brings the other machines' source changes and their dirs, and the activation after it renders this machine's.

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
```

`inputs` and `cache/` are safe to delete. Deleting `outputs` or `manifest` makes maw forget what it wrote, so drift goes unnoticed until the next activation.
