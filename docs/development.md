# Development

## Layout

```path
src/
  cli/            # command tree; each command is a thin wrapper over a library fn
  env.rs          # Env: every path maw touches
  runner.rs       # Runner: every external command
  inputs.rs       # file stamps and hashes
  eval.rs         # nix-instantiate calls and the eval cache
  registry.rs     # name + file role -> destination
  repo.rs         # locate and scaffold the dotfiles repo
  build.rs        # incremental build into out/
  activate.rs     # link plan against the manifest, drift, loose static/ files
  backup.rs       # moves files aside before maw replaces them
  state.rs        # maw.nix: read from nix, written in a fixed shape
  edit.rs         # edit, new (with live-file import), add
  status.rs       # status and diff, planned without writing
  help.rs         # maw help: the user docs, compiled in and rendered for the terminal
  packages.rs     # install and remove: plan, run the backend, record in maw.nix, scaffold modules
  backend/        # Backend and SystemBackend traits; xbps.rs, cargo.rs, go.rs, srcpkgs.rs (xbps-src builds)
  init/           # InitBackend trait and runit.rs: service files, links, sv control
  system.rs       # activation's root half: sudo copies, service enable/disable/restart
  services.rs     # maw sv: list, enable, disable, status, restart, log
  adopt.rs        # undeclared packages and services, the adopt checklist
  generations.rs  # auto-commit, the generations log, commit/push/pull
  rollback.rs     # restore a generation: repo, package versions, holds
  index.rs        # out/.maw/index.json: activating without nix
  scaffold/       # src new --from-nix: nixpkgs metadata -> SourcePkg -> xbps-src template (SourceEmitter)
  complete.rs     # tab completion: repo names for the dynamic completer, and the shell scripts
  style.rs        # the palette: tones by meaning, shared by the cli and the tui
  man.rs          # man pages: maw(1) and subcommands from clap, docs through markdown -> roff
  tui/            # `maw` alone: app.rs (state and keys, no io), data.rs (tab loaders), view.rs (drawing), term.rs (tty and fds), mod.rs (event loop)
nix/nixpkgs-meta.nix   # one nixpkgs package's metadata as plain data
depmap.nix        # shipped nixpkgs -> void dependency names, installed to /usr/share/maw
srcpkgs/maw/      # maw's own xbps-src template
man/              # generated man pages (committed)
completions/      # generated completion scripts (committed)
tools/scrape-registry/   # grows registry.nix from home-manager (a separate crate in the workspace)
  testing.rs      # shared unit-test fixture: tempdir repo over a fake nix
registry.nix      # shipped registry
nix/
  default.nix     # entry point: takes { dotfiles }, exposes modules.<name>
  lib.nix         # maw's lib: program, raw, generators
  nixpkgs-lib/    # pinned copy of nixpkgs lib/ (commit in REV)
tests/
  golden.rs       # golden tests for modules and generators
  build.rs        # end-to-end build of the fixture repo with real nix
  activate.rs     # end-to-end activate of the fixture repo, then a no-op rerun
  edit.rs         # imported configs render unchanged; added files link back in place
  scaffold.rs     # templates from real nixpkgs metadata (go, rust, meson) against goldens
  generated.rs    # man/ and completions/ match what maw generates
  scaffold/       # the metadata fixtures, void names, and expected templates
  fixtures/dotfiles/   # example dotfiles repo
  golden/<module>/<key>   # expected output per fixture module file
  generators/<name>.nix   # generator fixture, expected output in <name>.out
```

## Testing seams

Two seams keep everything testable without touching the machine:

- `Env` holds every path: home, system root, state, and maw's share dir. Tests build one over a tempdir.
- `Runner` runs every external command. Unit tests use `FakeRunner`, which records calls and answers with canned output.
- `Ask` asks the user a question. The CLI asks on the terminal; tests answer with a fixed string or nothing.
- `Runner::interactive` runs a command on the user's terminal, like the editor, and `Runner::pipe` feeds one text on stdin, like the pager. `FakeRunner` records both like any other call.

Package commands run `xbps-*` through `sudo` on the real system. With `MAW_SYSROOT` set they skip `sudo` and pass `-r <root>` instead, so a scratch root works without root privileges once it has repos and keys:

```sh
R=/tmp/maw-root; mkdir -p $R/var/db/xbps/keys $R/etc/xbps.d
cp /var/db/xbps/keys/* $R/var/db/xbps/keys/; cp /etc/xbps.d/*.conf /usr/share/xbps.d/*.conf $R/etc/xbps.d/
MAW_SYSROOT=$R maw install tzdata
```

A root without `var/db/xbps` counts as having nothing installed. Root copies and system services work the same way: with `MAW_SYSROOT` set, `install`, `ln`, and `sv` run without `sudo` inside the scratch root. To watch services really run there, start a supervisor over each dir yourself: `runsvdir $R/var/service &` and `runsvdir $HOME/.config/service &`.

Source builds are only ever faked in tests: the fixture pre-creates the clone's `xbps-src`, so nothing is cloned, and `xbps-src` calls are recorded rather than run.

cargo and go follow `HOME`, so a scratch `HOME` keeps their installs out of your real `~/.cargo` and go dir. rustup reads `HOME` too; point it back at your toolchains and unset any `GOPATH` from your shell:

```sh
export HOME=/tmp/maw-home RUSTUP_HOME=$OLDHOME/.rustup CARGO_HOME=/tmp/maw-home/.cargo
unset GOPATH GOBIN
```

Running the binary picks these up from the environment:

| variable | default | meaning |
|---|---|---|
| `HOME` | | home directory for destinations and state |
| `MAW_SYSROOT` | `/` | prefix for system paths like `/etc` |

The share dir is `/usr/share/maw` when installed, else the source checkout. To try maw without touching your real home, build first, then point `HOME` at a scratch dir. rustup reads `HOME` too, so run the built binary directly rather than `cargo run`:

```sh
cargo build
export MAW=$PWD/target/debug/maw HOME=/tmp/maw-home MAW_SYSROOT=/tmp/maw-home/sysroot
$MAW init ~/dots
```

## The TUI

TUI actions build a `cli::Command` and run it through `cli::run` on a background thread, the same function the CLI dispatches to. While the TUI is open it draws on `/dev/tty`, and the process's stdin is `/dev/null` while stdout and stderr go to a pipe, so everything commands print (maw's own lines, xbps, cargo) streams into the output pane. The CLI's two interactive points go through `cli::Hooks`: questions become popups, and the editor makes the TUI step aside and restore the real file descriptors until it exits. Root steps rely on a `sudo -v` taken when the TUI opens and renewed before an action if `sudo -n true` fails.

`app.rs` is plain state: key events in, `Request`s out, tested without a terminal; `view.rs` is tested by rendering into ratatui's `TestBackend`. The whole thing can be driven for real in a pty with timed keystrokes: `(sleep 3; printf 2; sleep 1; printf q) | script -qfc maw log`.

## Tests

Requires `nix-instantiate` (`xbps-install nix`).

```sh
cargo test                  # compare rendered output against goldens
MAW_BLESS=1 cargo test      # rewrite goldens from current output
```

After blessing, check the golden diff before committing.

## The registry

`registry.nix` is hand-maintained, grown by a scraper over home-manager, which knows where hundreds of programs keep their config:

```sh
git clone --depth 1 https://github.com/nix-community/home-manager /tmp/hm
cargo run -p scrape-registry -- /tmp/hm registry.nix
```

It reads each module in `modules/programs/` for literal `xdg.configFile."..."` (under `~/.config`) and `home.file."..."` (under `~/`) paths, skipping interpolated and macOS paths. It keeps programs Void packages (by name, from `xbps-query -Rs ""`) that the registry doesn't have yet (asked of nix, never parsed), picks the most config-like file as `main`, guesses `format` from its extension, and appends them under a `# from home-manager` marker. Existing entries are never touched, so hand fixes stay; move or fix appended ones freely, and rerunning only adds what's still missing. Modules whose literal paths aren't the program's own config are listed in `SKIP` in `tools/scrape-registry/src/main.rs`, with the reason.

## Colors

Every color maw shows comes from `src/style.rs`, the same for the CLI (on a terminal only; piped output stays plain) and the TUI. Pick a tone by meaning, never a raw color: `Accent` for titles, key letters, pending work, and additions; `Bad` for errors and removals; `Warn` for warnings and drift; `Dim` for borders, notes, and context. The selection is white on void's logo green. Bold is for titles, key letters, the selection, and the `error:`/`warning:` prefixes.

## Generated files

`man/` (maw.1, a page per subcommand, and the docs as sections 5 and 7) and `completions/` (bash, zsh, fish) are generated by `src/man.rs` and `src/complete.rs` and committed, so the package build needs nothing extra. `tests/generated.rs` fails when they're stale; `MAW_BLESS=1 cargo test` rewrites them, like the goldens. Man pages come from the command definitions (clap_mangen) and from the same markdown `maw help` embeds, through a small markdown-to-roff writer. Completion scripts only register maw with the shell: each asks `COMPLETE=<shell> maw -- <words>` for candidates, answered by `clap_complete`'s dynamic engine and the completers in `complete.rs`, which read the repo, `out/.maw/index.json`, and the generations log, never nix.

## Releasing

maw ships as an xbps package built from `srcpkgs/maw/template` (`build_style=cargo`). It installs the binary, plus `nix/` and `registry.nix` into `/usr/share/maw`, where the installed maw looks for them. To release a version:

```sh
tools/release.sh 0.2.0
git push --follow-tags
```

`tools/release.sh` bumps the version in `Cargo.toml`, `Cargo.lock` and `mawVersion` in `nix/lib.nix`, commits, and tags `v<version>`. Pushing the tag runs the release workflow (`.github/workflows/release.yml`), which checksums GitHub's tarball of the tag and commits the template's new `version` and `checksum` to master; `git pull` afterwards. The workflow refuses a tag whose `Cargo.toml` doesn't carry its version. The repo has to be public, since xbps-src fetches the tarball without credentials.

It also installs the shipped `depmap.nix`. To install it through maw itself, copy `srcpkgs/maw/` into your dotfiles' `srcpkgs/` and `maw install maw`; after each release, fetch the updated template and `maw sync` rebuilds and upgrades maw:

```sh
curl -fsSLo ~/dotfiles/srcpkgs/maw/template https://raw.githubusercontent.com/AamindMandragora/maw/master/srcpkgs/maw/template
maw sync
```

## Docs

`docs/usage.md`, `docs/migrating.md`, `docs/modules.md`, and `docs/formats.md` are compiled into the binary and served by `maw help`, so they're user-facing twice: keep them readable as plain text, with headings that make sense as `maw help <heading>`. Link between them as `[formats.md](formats.md)`; the terminal renders that as `maw help formats`. A test checks that every command's `see:` line names a real heading. This file isn't compiled in.

## Pinned nixpkgs lib

`nix/nixpkgs-lib/` is a plain copy of nixpkgs `lib/`, minus its tests. It's vendored, not a submodule, so maw installs as a single package with no network access at build time. To update it:

```sh
git clone --depth 1 --filter=blob:none --sparse https://github.com/NixOS/nixpkgs /tmp/nixpkgs
git -C /tmp/nixpkgs sparse-checkout set lib
rm -rf nix/nixpkgs-lib && cp -r /tmp/nixpkgs/lib nix/nixpkgs-lib && rm -rf nix/nixpkgs-lib/tests
git -C /tmp/nixpkgs rev-parse HEAD > nix/nixpkgs-lib/REV
cargo test
```
