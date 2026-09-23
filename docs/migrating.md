# Migrating

How to move a machine you already configured by hand, or with stow or a bare git repo, into maw. Nothing on disk changes until the final `maw activate`, and everything it replaces is backed up first.

## 1. Create the repo

```sh
maw init ~/dotfiles
```

This works on an existing directory too; maw only adds what's missing.

## 2. Bring in configs

Decide per program whether it becomes a module (Nix you can share values between) or a verbatim file in `static/`. A module is worth it for programs whose config you want to tie together, like fonts and colors across a bar, launcher, and terminal. Everything else can stay verbatim.

Use `--no-activate` throughout, so you can assemble the whole repo and activate once at the end.

### As modules

```sh
maw new niri --no-activate
maw new waybar --no-activate
```

`new` imports whatever config is already where the module will put it, wrapped in `lib.raw`, so the module renders byte for byte what you have. Convert it to Nix later, a section at a time; see `maw help modules`.

A program the registry doesn't know writes to `~/.config/<name>/config`. If that's wrong for it, `maw add` its file instead (below), which records the real path.

### As verbatim files

```sh
maw add ~/.config/nvim --no-activate          # a whole dir, file by file
maw add ~/.gitconfig --no-activate
maw add ~/.local/bin/powermenu --no-activate
```

Each copy lands back exactly where it came from; see "Adding verbatim files" in `maw help usage`.

### What to leave out

- Files an app rewrites on its own, like a GUI's settings file or a plugin lockfile. Linked, they turn into drift every time the app saves.
- Anything holding secrets or tokens (`gh/hosts.yml`, `rclone.conf`, `.claude.json`). The repo is meant to be committed and pushed.
- Configs for programs you no longer have installed.

## 3. Check before touching anything

```sh
maw status               # every file that would change
maw diff                 # live content against what maw would put there
maw activate --dry-run   # the exact plan
```

Raw imports and `add`ed files diff to nothing: same content, just about to become a link. `status` shows them as `blocked`, meaning a real file sits where the link goes and will be backed up.

A module you converted to Nix may differ in formatting (key order, spacing) but should say the same thing. Check with a parser where there is one, or the program's own validator, e.g. `niri validate -c ~/dotfiles/out/niri/config.kdl`.

## 4. Activate

```sh
maw activate
```

Each replaced file is moved to `~/.local/state/maw/backups/`, under the same path relative to your home (system files under `backups/system/`). A second `maw activate` should print `up to date`, and `maw status` should print `clean`.

If you were using a bare git repo or stow, retire it now: the files it tracked are links into `~/dotfiles` from here on, so it will show them all as changed.

## 5. Packages and services

Record what's installed and enabled, so a fresh machine gets it too:

```sh
maw adopt
```

It opens a checklist of every package you installed by hand and every enabled service; keep what the machine should have, skip the rest (maw remembers the skips). See `maw help adopting`. Afterwards `maw status` should print `clean`, apart from `orphan` lines for any config whose program you removed.

### Moving a service you wrote

A service you wrote by hand in `/etc/sv/<name>` can become a module. If it only drops to your user (with `chpst -u you`) to do its job, it's usually better as a user service, which runs as you from login and needs no `chpst`:

1. Write `modules/<name>.nix` with `lib.service "<name>" { scope = "user"; run = ''...''; }`, pasting the script without the privilege-dropping lines. User services already have `HOME`, `USER`, and `PATH` set.
2. Check the rendered script: `sh -n ~/dotfiles/out/sv/<name>/run`.
3. Stop the old one first, so both never run at once: `maw sv disable <name> --system`.
4. `maw activate` links and starts the new one. Follow it with `maw sv log <name>`.
5. Once it works, remove the old definition: `sudo rm -r /etc/sv/<name>`. maw never deletes a service definition it didn't write.

### System files

Config under `/etc`, like greetd's, is copied with `sudo` rather than linked. `maw new greetd` imports it like any other module. The first activation backs up the original to `~/.local/state/maw/backups/system/`.

## Undoing it

A single file: delete its link and move its backup back.

```sh
rm ~/.config/foot/foot.ini
mv ~/.local/state/maw/backups/.config/foot/foot.ini ~/.config/foot/foot.ini
```

A module or static file you remove from the repo is unlinked on the next `maw activate`; its backup stays where it is, so restore it the same way.
