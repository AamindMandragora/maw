# 11. Machines

One repo can serve several machines that are mostly alike, like a laptop and a desktop. Each machine has a name of letters, digits, `-`, `_`, and `.`: `maw init` asks for it (your hostname by default), and `maw host` shows it, or renames the machine and activates for what the new name gets (without a generation; if that activation fails, the old name stays):

```sh
maw host            # laptop, with hosts/laptop.nix
maw host desktop    # this machine is now desktop
```

The name is kept in `~/.config/maw/host`, and each machine renders into `out/<name>/`. What differs between machines goes in three places:

- **Config values**: `hosts/<name>.nix` is merged over `config.nix` on that machine only, key by key (attrsets merge; a list or any other value replaces the one in `config.nix`), so it holds just what differs:

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

Next: chapter 12, history (`maw help 12`).
