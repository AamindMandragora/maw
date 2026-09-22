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
registry.nix      # shipped registry
nix/
  default.nix     # entry point: takes { dotfiles }, exposes modules.<name>
  lib.nix         # maw's lib: program, raw, generators
  nixpkgs-lib/    # pinned copy of nixpkgs lib/ (commit in REV)
tests/
  golden.rs       # golden tests for modules and generators
  build.rs        # end-to-end build of the fixture repo with real nix
  activate.rs     # end-to-end activate of the fixture repo, then a no-op rerun
  fixtures/dotfiles/   # example dotfiles repo
  golden/<module>/<key>   # expected output per fixture module file
  generators/<name>.nix   # generator fixture, expected output in <name>.out
```

## Testing seams

Two seams keep everything testable without touching the machine:

- `Env` holds every path: home, system root, state, and maw's share dir. Tests build one over a tempdir.
- `Runner` runs every external command. Unit tests use `FakeRunner`, which records calls and answers with canned output.
- `Ask` asks the user a question. The CLI asks on the terminal; tests answer with a fixed string or nothing.

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

## Pinned nixpkgs lib

`nix/nixpkgs-lib/` is a plain copy of nixpkgs `lib/`, minus its tests. It's vendored, not a submodule, so maw installs as a single package with no network access at build time. To update it:

```sh
git clone --depth 1 --filter=blob:none --sparse https://github.com/NixOS/nixpkgs /tmp/nixpkgs
git -C /tmp/nixpkgs sparse-checkout set lib
rm -rf nix/nixpkgs-lib && cp -r /tmp/nixpkgs/lib nix/nixpkgs-lib && rm -rf nix/nixpkgs-lib/tests
git -C /tmp/nixpkgs rev-parse HEAD > nix/nixpkgs-lib/REV
cargo test
```
