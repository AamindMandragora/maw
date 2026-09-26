# Modules

A dotfiles repo looks like this:

```path
dotfiles/
  default.nix    # import <maw> { dotfiles = ./.; }
  config.nix     # your settings: fonts, colors, anything modules share
  maw.nix        # written by maw: packages, services, recorded paths
  modules/       # one <name>.nix per program
```

## A module

A module is a function that returns the files for one program. It names the program, never a path. maw works out where the file goes.

```nix
{ config, lib, ... }:
lib.program "foot" {
  format = "ini";
  settings = {
    main.font = "${config.font.mono}:size=${toString config.font.size}";
    colors = config.colors.foot;
  };
}
```

The arguments:

- `config`: the contents of `config.nix`. It can be an attrset, or a function taking `{ lib }`, `{ theme }`, or both.
- `lib`: the nixpkgs lib plus maw's additions (below).
- `maw`: the contents of `maw.nix`.
- `theme`: the palette from the current wallpaper, `{ }` before there is one; see "Wallpaper themes" in `maw help usage`.

A module takes the arguments it names; list only the ones you use.

## `lib.program name { ... }`

| option | default | meaning |
|---|---|---|
| `format` | `"raw"` | how to render `settings`; see [formats.md](formats.md) |
| `settings` | | the file's contents as Nix values |
| `files` | `{ main = settings; }` | several files, keyed by role |
| `executable` | `false` | mark the output executable |
| `path` | | a subpath for the main file, when the program needs one |
| `scope` | `"user"` | `"user"` or `"root"` |
| `reload` | the registry's | a command run after the files change, so the running program reads them |

Programs with more than one file use `files`. `format` can then be one string for all files, or an attrset with one format per file:

```nix
lib.program "waybar" {
  format = { main = "json"; style = "css"; };
  files = {
    main = { layer = "top"; modules-right = [ "clock" ]; };
    style."window#waybar".background = "#1e1e2e";
  };
}
```

A plain string or `lib.raw` as a file's settings is written verbatim, whatever the format.

## `lib.service name { ... }`

Describes a runit service. maw writes its `run` and `log/run` files and enables it; see "Services" in `maw help usage`.

| option | default | meaning |
|---|---|---|
| `scope` | `"system"` | `"system"` runs from boot as root, `"user"` runs as you from login |
| `run` | | the script: shell text, or a whole script with its own `#!` line |
| `log` | `true` | log through svlogd to `/var/log/<name>/` or `~/.local/state/log/<name>/` |
| `enable` | `true` | link it into place; `false` writes the definition but leaves it off |
| `env` | `{ }` | variables exported before `run`, literally: no `$` expansion |

```nix
{ config, lib, ... }:
lib.service "rclone" {
  scope = "user";
  env.RCLONE_VFS_CACHE_MODE = "full";
  run = ''
    exec rclone --config "$HOME/.config/rclone/rclone.conf" mount "Google Drive:" "$HOME/Google Drive"
  '';
}
```

The run script gets `exec 2>&1` first, so errors reach the log, then the `env` exports. It should end by `exec`ing the long-running program, which runit supervises and restarts when it exits. A module can return a service alongside program files as a list: `[ (lib.program "x" { ... }) (lib.service "x" { ... }) ]`.

## Reloading

When an activation changes a program's files, maw runs its reload command once all files are in place: `pkill -USR2 -x waybar`, `dunstctl reload`, `makoctl reload`, and so on. The registry knows the command for common programs; `reload` in `lib.program` sets or overrides it:

```nix
lib.program "waybar" {
  reload = "pkill -USR2 -x waybar";
  ...
}
```

The command runs through `sh -c` as you. A program that isn't running fails its reload, which is fine. Programs that watch their own config, like niri and alacritty, need none. Services restart on their own instead; see `lib.service`.

## `lib.dconf { ... }`

Desktop settings: the dconf database that GTK apps, GNOME, and `gsettings` read. Keys are grouped by path:

```nix
{ lib, ... }:
lib.dconf {
  "org/gnome/desktop/interface" = {
    color-scheme = "prefer-dark";
    gtk-theme = "Adwaita-dark";
    font-name = "Adwaita Sans 11";
    text-scaling-factor = 1.25;
  };
}
```

Strings, numbers, booleans, and lists become GVariant values; for anything else (`uint32 5`, tuples), write the GVariant text with `lib.raw`. A module can return settings next to program files as a list: `[ (lib.program "x" { ... }) (lib.dconf { ... }) ]`. Two modules setting one key to different values is an error.

maw writes each key with `dconf write`, and only when its value differs. A key you change by hand afterwards, say in a settings app, is reported and left alone, like a hand-edited file; `maw activate --force` puts the declared value back. Keys you stop declaring are reset to their defaults, unless you've changed them since. dconf needs a desktop session: activating from a bare console skips settings with a note, and the next activation in your session applies them.

## Verbatim text

`lib.raw ''...''` is accepted anywhere a value is. Use it where Nix syntax doesn't reach, and mix it freely with Nix in one file:

```nix
settings = {
  main.pad = "8x8";
  bindings = lib.raw ''
    [key-bindings]
    clipboard-copy=Control+Shift+c
  '';
};
```

## Ordering

Nix sorts attrset keys alphabetically, so rendered files come out sorted. When order matters, like niri window rules or CSS cascade, use a list.

## Sharing values

Anything several modules use, like colors, fonts, or your terminal, belongs in `config.nix`:

```nix
{
  font.ui = "Adwaita Sans";
  colors.text = "f2f2f7";
  terminal = "ghostty";
}
```

Then `config.colors.text` in fuzzel, waybar, and niri all read the same value. With a [wallpaper theme](usage.md), colors can come from the palette instead, in one place:

```nix
{ lib, theme }:
{
  colors.text = lib.removePrefix "#" theme.colors.on_surface;
  colors.accent = lib.removePrefix "#" theme.colors.primary;
}
```

 Plain `let` bindings and functions work too, for repetition inside one module:

```nix
let
  workspaceBinds = prefix: action:
    lib.listToAttrs (map (n: lib.nameValuePair "${prefix}${toString n}" { ${action} = n; }) (lib.range 1 9));
in
# ...
binds = workspaceBinds "Mod+" "focus-workspace" // { "Mod+Q".close-window = null; };
```


`tests/fixtures/dotfiles/modules/` has complete real-world modules for niri, waybar, and fuzzel.

## Rendering a module by hand

`-I maw=` points at maw's `nix/` directory:

```sh
nix-instantiate --eval --strict --raw -I maw=./nix \
  -E '(builtins.head (import ./path/to/dotfiles).modules.fuzzel).content'
```

Each module evaluates to a list of `{ name, key, content, executable, scope }`, one entry per file.
