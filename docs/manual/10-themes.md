# 10. Themes

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

An image has a few candidate colors a palette can grow from, most dominant first; each `maw wallpaper` picks one at random, so the same wallpaper can come back in a different color. Then maw activates, so modules that use the theme rebuild and their programs reload. Changing the wallpaper isn't a generation, and leaves nothing to commit. A file that uses the theme is rendered twice in the same evaluation: once with the real palette, placed from `~/.local/state/maw/themed/<machine>/`, and once with every color a placeholder gray (`#808080`) and the wallpaper as `<wallpaper>`, which is the copy `out/` and git keep. A new wallpaper changes only the first, so `git diff` shows only real config changes. `maw build` lists themed files as `write themed/<file>`. A machine activating with `--no-build` before its first build places the placeholder copy until then.

Because the placeholder render has the same palette keys with gray values, a module can read `theme.colors.primary` or `theme.wallpaper` directly; `theme` is only `{ }` before the first wallpaper.

In `config.nix`, the palette's settings, with their defaults (setting `maw.theme` also themes a fresh machine from the first wallpaper):

```nix
maw.theme = {
  mode = "dark";                  # or "light"
  scheme = "scheme-tonal-spot";   # or scheme-vibrant, scheme-expressive, scheme-fidelity, scheme-content, ...
  contrast = 0;                   # -1 to 1
};
```

Modules and `config.nix` read it as `theme` (see [modules.md](04-modules.md)):

- `theme.colors`: Material You colors as `#rrggbb`: `primary`, `on_primary`, `surface`, `on_surface`, `surface_container`, `outline`, `error`, `secondary`, `tertiary`, and the rest of the scheme
- `theme.base16`: `base00` through `base0F`, for programs with base16 themes
- `theme.wallpaper`: the image's absolute path
- `theme.mode`, `theme.source`: the mode and the color the palette grew from

To show the wallpaper, declare it like anything else. A service is best: the path is in its run file, so changing the wallpaper restarts it (your compositor has to share its display with user services; see `lib.service` in [modules.md](04-modules.md)):

```nix
{ lib, theme, ... }:
lib.service "swaybg" {
  scope = "user";
  run = "exec swaybg -i ${theme.wallpaper} -m fill";
}
```

To change it on a timer, run `maw wallpaper random` from anything that runs on a schedule, like a user service that sleeps between changes. Theming needs `matugen` (`maw install matugen`); maw itself doesn't depend on it.

Next: chapter 11, machines (`maw help 11`).
