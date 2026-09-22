# maw docs

maw manages a Void Linux machine from one git repo: packages, runit services, and config files. You describe config in Nix, maw renders it to each program's native format and puts it in place.

maw only uses Nix as a config language. It never builds or installs anything from the Nix store.

## Pages

- [usage.md](usage.md): setting up a repo, building, where files go
- [modules.md](modules.md): writing a module for a program
- [formats.md](formats.md): every output format and how Nix values map to it
- [development.md](development.md): repo layout, tests, the pinned nixpkgs lib
