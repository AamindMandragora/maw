# 6. Packages

Packages you want on the machine are declared in `maw.nix`, one list per source:

```nix
packages = {
  xbps = [ "foot" "niri" ];
  flatpak = [ "com.slack.Slack" "org.gnome.Calculator" ];
  cargo = [ "bat" "typos-cli@1.24" "https://github.com/user/tool.git" ];
  go = [ "github.com/jesseduffield/lazygit" ];
  uv = [ "ruff" "httpie@3.2.3" ];
  npm = [ "prettier" ];
};
```

maw writes those lists; you change them with the commands below. `maw activate` installs anything declared but missing, before it links files. Nothing is ever removed just because it isn't declared.

## Installing

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

Without a terminal it stops and names the prefixes to use instead. When no source has the name, maw looks in the AUR and nixpkgs and offers to [draft a template](07-source-packages.md) from one; `maw install aur:<name>` or `nixpkgs:<name>` goes straight there. The draft opens in your editor, and `maw install <name>` then builds and installs it. Library crates and npm packages with no commands are refused before any question, since there's nothing to install; add them to a project with `cargo add` or `npm install` instead. A `flatpak:`, `cargo:`, `go:`, `uv:`, `npm:`, or `xbps:` prefix skips all of that. Python and npm packages always need their prefix. `@version` pins a crate, go program, python program, or npm package to that version; without it you get the latest.

Each source needs its tool: `cargo`, `go`, `uv`, `flatpak`, or `npm` (Void's `nodejs`). When one is missing, maw stops and says what to install: `error: uv isn't installed; maw install uv first`. `maw install uv uv:ruff` does both, xbps first.

Then maw installs (`sudo xbps-install`, `sudo flatpak install`, `cargo install --locked`, `go install`, `uv tool install`, or `npm install -g`), records the package in `maw.nix`, and, when the registry knows the program and the repo has no module or `static/` dir for it yet, creates its module the way `maw new` does, importing any config already on disk. Then it activates. A package that's already installed is only recorded. A name nothing has is an error.

cargo installs into `~/.cargo/bin`, go into `$GOBIN` (or `$GOPATH/bin`, or `~/go/bin`), and uv and npm into `~/.local/bin` (npm with `~/.local` as its global prefix, so nothing needs root). maw leaves your shell config alone, but warns after an install if that dir isn't on your `PATH`, naming the line to add.

### Python programs

uv gives each python program its own environment. A program that needs an older Python, or an extra package in its environment, gets them in `config.nix`, by its name:

```nix
maw.uv.openconnect-sso = {
  python = "3.12";                     # uv downloads it if the system has another
  requirements = [ "setuptools<81" ];  # installed alongside, like uv tool install --with
};
```

They apply whenever maw installs or upgrades it: `maw install uv:openconnect-sso`, and `maw sync`.

### Flatpak apps

Flatpaks come from Flathub (maw adds the remote if it's missing) and are installed system-wide, for every user. Each declared app also gets a wrapper in `~/.local/bin` named after the last part of its id, so configs and shells can run it by that name: `calculator` runs `flatpak run org.gnome.Calculator`. To pick another name, in `config.nix`:

```nix
maw.flatpakNames = { "us.zoom.Zoom" = "zoom-meet"; };
```

App launchers like fuzzel find flatpaks through their desktop entries either way. `@<commit>` pins an app to an exact build, which is how generations record them. An app's permissions and environment are declared with `lib.flatpak` in a module; see [modules.md](04-modules.md).

## Removing

```sh
maw remove foot
maw remove bat          # a crate, found by name
maw remove lazygit      # a go program, by its binary or path
maw remove calculator   # a flatpak, by its id or wrapper name
```

Removes the package and drops it from `maw.nix`. xbps also removes dependencies nothing else needs, and flatpak runtimes nothing uses any more; go programs are deleted from the bin dir. Its module stays, so reinstalling brings the config back; delete `modules/foot.nix` yourself if you're done with it. `--dry-run` prints the plan.

## Looking things up

```sh
maw query           # packages you installed or declared, in every source
maw query bat       # one of them
maw search term     # every source; [*] installed, [~] draftable
maw search npm:term # one source: xbps, flatpak, cargo, go, uv, npm, aur, or nixpkgs
maw info bat        # details, plus whether maw manages it
```

`query` lists what you installed by hand plus everything declared, flagging the two kinds of mismatch: `(undeclared)` for installed but not in `maw.nix`, `(missing)` for declared but not installed. Crates and go programs are marked with their source. `search` looks in xbps, Flathub, crates.io, PyPI (exact names only, since PyPI has no search), and npm, and prints names the way `maw install` takes them. AUR and nixpkgs matches come last, marked `[~]`: they can't be installed as they are, only drafted into a template. A source whose tool isn't installed is skipped; one that doesn't answer gets a warning. nixpkgs is searched through [search.nixos.org](https://search.nixos.org). `info` shows the version, whether it's installed and declared, the module or `static/` dir holding its config, and where the registry puts that config.

## Updating

```sh
maw sync
```

Upgrades the system (`xbps-install -Su`), then every flatpak, crate, go, python, and npm program that isn't pinned to a version. Then it moves each [source package](07-source-packages.md) template that follows releases to its upstream's newest, and rebuilds every source package whose template is ahead of what's installed. That's how maw updates itself: its template follows maw's releases. Moved templates are repo changes, so sync then activates and records them as a generation. A source package that fails to build is a warning, and the rest of sync goes on; sync exits with an error at the end, naming what didn't build. Pinned packages stay put until you install a different version, and so do xbps packages a [rollback](12-history.md#rolling-back) held back; `maw sync --release` releases those first.

A template follows releases when it downloads a tagged release from GitHub, GitLab, Codeberg, or sourcehut with `${version}` in the url, like `distfiles="${homepage}/archive/refs/tags/v${version}.tar.gz"`. maw reads the repo's tags (`git ls-remote`), takes the newest plain version matching the url's tag (`v${version}` finds `v0.3.0`, skipping pre-releases like `v0.3.0-rc1`), and rewrites `version`, `revision`, and `checksum`. `maw src update <name>` does the same for one template. A `# maw: hold` line keeps a template where it is.

## Adopting

```sh
maw adopt
maw adopt --dry-run    # print the checklist instead
```

Lists everything `status` calls `undeclared` in your editor, one line each, every line starting as `keep`:

```
# maw adopt: `keep` records it in maw.nix, `here` for this machine only, `skip` (or deleting the line) ignores it from now on

# packages (xbps)
keep firefox
skip base-devel

# packages (cargo)
keep https://github.com/user/tool.git

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

Next: chapter 7, source packages (`maw help 7`).
