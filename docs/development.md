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
srcpkgs/maw/      # maw's own xbps-src template
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

## Tests

Requires `nix-instantiate` (`xbps-install nix`).

```sh
cargo test                  # compare rendered output against goldens
MAW_BLESS=1 cargo test      # rewrite goldens from current output
```

After blessing, check the golden diff before committing.

## Releasing

maw ships as an xbps package built from `srcpkgs/maw/template` (`build_style=cargo`). It installs the binary, plus `nix/` and `registry.nix` into `/usr/share/maw`, where the installed maw looks for them. To release a version:

1. bump `version` in `Cargo.toml` and in the template, and `mawVersion` in `nix/lib.nix`
2. tag the commit `v<version>` and push the tag
3. `xgensum -i srcpkgs/maw/template` to fill in the checksum, and `xlint` it

To install it through maw itself, copy `srcpkgs/maw/` into your dotfiles' `srcpkgs/` and `maw install maw`; from then on, updating the template's version there and running `maw sync` rebuilds and upgrades maw.

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
