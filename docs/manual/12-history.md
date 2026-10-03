# 12. History

Every activation that changes something is a generation: maw commits your whole repo (modules, static files, `config.nix`, `maw.nix`, and `out/`) and records it. Changes you made by hand count too, even ones nothing places, like a wallpaper dropped into `static/wallpapers/` or a note in the repo; the message names their folders (`generation 13: static/wallpapers`). The same happens after `install`, `remove`, `sv enable`, `sv disable`, `adopt`, and `pull`, since each ends by activating.

Before committing, maw asks for a message, offering a generated one:

```
commit message [generation 12: niri, waybar; 2 links; install foot]:
generation 12 (3f9a2c1)
```

Press enter to take it. Without a terminal the generated one is used.

```sh
maw generations
```

```
  11  2026-09-22 21:04  e761146  initial dotfiles
  12  2026-09-23 14:02  3f9a2c1  generation 12: niri, waybar; 2 links; install foot
```

Each generation also records the exact version of every installed package in every source, in `~/.local/state/maw/generations`. That log belongs to the machine, not the repo: two machines sharing a repo each have their own.

To skip committing once, `maw activate --no-commit`. To turn it off, in `config.nix`:

```nix
maw.autoCommit = false;
```

To commit by hand, e.g. edits you haven't activated yet:

```sh
maw commit -m "try a lighter bar"
maw commit            # git opens your editor for the message
```

## Rolling back

```sh
maw rollback            # to the generation before the latest
maw rollback 11         # to generation 11
maw rollback --dry-run
```

```
restore generation 11 (e761146)
remove cargo:bat
install libportal-0.10.0_1
keep firefox: firefox-150.0_1 isn't in the cache, binpkgs, or the repo
hold libportal
```

A rollback is itself a new generation, so history only moves forward and you can roll back a rollback. It:

1. restores the repo to that generation's commit (it needs no uncommitted changes, so `maw commit` first): everything shared, and this machine's own `maw.nix` lists, `hosts/<name>` files, and `out/<name>/`; other machines' parts stay as they are, so a rollback here never undoes their changes. An encrypted file taken back to an earlier version is named, since it may not be encrypted for machines added since (`maw secret rekey` fixes that),
2. removes packages declared now but not then, and puts every package declared then back at the version that generation recorded,
3. activates, which relinks config and re-enables services the way they were.

Only declared packages change; ones you installed by hand and never adopted are left alone. An old xbps version comes from xbps's download cache in `/var/cache/xbps` (which xbps keeps unless you clean it), from an earlier build of your own in the void-packages clone's `hostdir/binpkgs`, or from the repo if it's still current there. A version found in none of them stays as it is, and the plan says so. Flatpaks go back to their recorded commit, and everything else is reinstalled at its recorded version.

xbps packages left behind the repo's newest version are held, so `maw sync` doesn't undo the rollback. They stay held until `maw sync --release`, or until a later rollback puts them back at the newest version.

## Sharing

```sh
maw push      # to the repo's remote, setting it as upstream
maw pull      # bring in the remote's commits, then activate
```

Both need a remote: `git -C ~/dotfiles remote add origin <url>`. Once there is one, every commit maw makes is pushed right after, so you rarely run `maw push` yourself. A push never stops to ask for a password; one that fails, offline or because another machine pushed first, is a warning, and the commit waits for the next push. To turn pushing off, in `config.nix`:

```nix
maw.autoPush = false;
```

`pull` brings the remote's commits in. Commits this machine has that the remote doesn't, like ones made offline, go back on top of them. When the remote's history was rewritten by a [squash](#squashing) on another machine, the pull says so, keeps this machine's old history on a branch, and puts its unpushed commits on top of the new one:

```
the remote's history was rewritten (a squash)
move 1 unpushed commit on top
keep the old history on branch maw-backup/pull-3f9a2c1; `git branch -D maw-backup/pull-3f9a2c1` once you don't need it
```

An unpushed commit that conflicts with the new history isn't lost: it stays on that branch, and the pull says how to apply it. `pull` needs a clean repo; `maw commit` first.

Each machine renders into its own `out/<machine>/`, so machines never rewrite each other's output: a pull brings the other machines' source changes and their dirs, and the activation after it renders this machine's. A file in `out/` you edited through its link is copied to the backups dir before a pull or a [rollback](#rolling-back) replaces it (`backup out/laptop/foot/foot.ini -> ~/.local/state/maw/backups/...`).

## Squashing

Generations pile up, and some of them are experiments that didn't work. `squash` folds a range of them, both ends included, into one commit:

```sh
maw squash 5 10
```

```
squash generations 5 6 7 8 9 10 (14 commits) into one; 3 later commits move on top
this rewrites history: the remote is force-pushed, and other machines' `maw pull` puts their unpushed commits on top
squash? [y/N] y
generation 10 is generations 5-10; the old history is on branch maw-backup/squash-5-10
push
```

The folded commit has exactly the files generation 10 had, and every commit after it moves on top unchanged. `maw generations` then lists generation 10 as `generations 5-10`, so rolling back to it still works; 5 to 9 are gone from the list, and from rolling back. Built packages only those generations recorded are freed at the next source build. Squashing from generation 1 starts the history there.

Before rewriting anything, maw fetches, and refuses if another machine pushed commits this one hasn't pulled: pull first, so they're kept. It also needs a clean repo. The old history stays on the `maw-backup/` branch until you delete it with `git branch -D`; `--yes` skips the question.

Next: chapter 13, migrating (`maw help 13`).
