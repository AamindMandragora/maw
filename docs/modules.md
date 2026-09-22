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

- `config`: the contents of `config.nix`. It can be an attrset, or a function taking `{ lib }`.
- `lib`: the nixpkgs lib plus maw's additions (below).
- `maw`: the contents of `maw.nix`.

## `lib.program name { ... }`

| option | default | meaning |
|---|---|---|
| `format` | `"raw"` | how to render `settings`; see [formats.md](formats.md) |
| `settings` | | the file's contents as Nix values |
| `files` | `{ main = settings; }` | several files, keyed by role |
| `executable` | `false` | mark the output executable |
| `path` | | a subpath for the main file, when the program needs one |
| `scope` | `"user"` | `"user"` or `"root"` |

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

Then `config.colors.text` in fuzzel, waybar, and niri all read the same value. Plain `let` bindings and functions work too, for repetition inside one module:

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
