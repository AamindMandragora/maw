# Formats

Set with `format = "<name>"` in `lib.program`, or call the generator directly (`lib.toCSS { ... }`). Every format accepts `lib.raw` at any node.

Floats print in their shortest form (`0.5`). When you build a string yourself, use `builtins.toJSON 0.5` rather than `toString 0.5`, which gives `0.500000`.

## ini (`lib.toINI`)

Attrsets become `[sections]`. Scalars at the top level are written first, without a section. Lists repeat the key. A `lib.raw` in a section's place is written verbatim, with no header.

```nix
{ shell = "bash"; main.term = "xterm-256color"; }
```
```ini
shell=bash

[main]
term=xterm-256color
```

## keyValue (`lib.toKeyValue`)

`key=value` lines. Lists repeat the key.

## json (`lib.toJSON`)

Indented JSON. A `lib.raw` value is spliced in as literal JSON.

## kdl (`lib.toKDL`)

For niri. The shape of each value decides the node:

| Nix | KDL |
|---|---|
| `gaps = 16;` | `gaps 16` |
| `spawn = [ "foot" "-e" "htop" ];` | `spawn "foot" "-e" "htop"` |
| `layout = { gaps = 16; };` | `layout { gaps 16 }` |
| `prefer-no-csd = null;` | `prefer-no-csd` |
| `spawn-at-startup = [ [ "waybar" ] [ "mako" ] ];` | the node twice, once per list |
| `window-rule = [ { ... } { ... } ];` | the block twice, in list order |
| `output = lib.kdl.node [ "eDP-1" ] { } { scale = 1.5; };` | `output "eDP-1" { scale 1.5 }` |

`lib.kdl.node args props children` covers anything else, like binds with properties:

```nix
"Mod+Shift+Slash" = lib.kdl.node [ ] { hotkey-overlay-title = "help"; } { show-hotkey-overlay = null; };
```

A node repeated with one argument each needs a list of lists, since a flat list is read as several arguments:

```nix
preset-column-widths.proportion = map (width: [ width ]) [ 0.25 0.5 0.75 ];
```

For property-only nodes like `match app-id="firefox"`, a small helper keeps modules readable:

```nix
props = attrs: lib.kdl.node [ ] attrs { };
window-rule = [ { match = props { app-id = "firefox"; }; open-maximized = true; } ];
```

A `lib.raw` in a node's place is written verbatim, and isn't re-indented.

## css (`lib.toCSS`)

Selectors map to properties. Nested attrsets are nested selectors: plain keys become descendants, and `&` stands for the parent. Keys starting with `@` that hold attrsets become blocks. Scalars at rule level become statements, and statements are written before rules.

```nix
{
  "@define-color bg" = "#1e1e2e";
  "*".font-family = [ "\"JetBrains Mono\"" "monospace" ];
  "window#waybar" = {
    background = "@bg";
    "#clock".padding = "0 8px";
    "&.hidden".opacity = 0;
  };
  "@media (max-width: 800px)"."#clock".font-size = "10px";
}
```
```css
@define-color bg #1e1e2e;

* {
  font-family: "JetBrains Mono", monospace;
}

@media (max-width: 800px) {
  #clock {
    font-size: 10px;
  }
}

window#waybar {
  background: @bg;
}

window#waybar #clock {
  padding: 0 8px;
}

window#waybar.hidden {
  opacity: 0;
}
```

When cascade order matters, pass a list of rule sets instead of one attrset. Each set is rendered in turn:

```nix
[
  { "#clock, #cpu".color = "#bababf"; }
  { "#clock".color = "#f2f2f7"; }   # must come after the group rule
]
```

A key can hold several selectors (`"#clock, #cpu"`), so a helper can share properties:

```nix
each = selectors: properties: { ${lib.concatStringsSep ",\n" selectors} = properties; };
```

Property lists are joined with `, `. Values are written as-is, so quote font names yourself when they need it. Nesting under a comma selector (`"a, b"`) isn't expanded per selector; write those rules out flat.

## shell (`lib.toShell`)

For bashrc-style files. Takes `{ aliases; exports; extra; }`:

```nix
{
  aliases.ll = "ls -l";
  exports.EDITOR = "nvim";
  exports.PATH = lib.raw ''"$HOME/.local/bin:$PATH"'';
  extra = lib.raw "PS1='\\w \\$ '\n";
}
```

Values are single-quoted. Use `lib.raw` to keep `$` expansion.

## raw

The default format. `settings` is a string written as-is.
