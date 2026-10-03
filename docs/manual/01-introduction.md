# 1. Introduction

maw manages a Void Linux machine from one git repo: packages, runit services, and config files. You describe config in Nix, and maw renders it to each program's native format and puts it in place. Nix is only the config language: maw never builds or installs anything from the Nix store.

This manual is in chapters, meant to be read in order the first time and searched after:

```
 1 introduction       what maw is, getting help, the tui
 2 getting started    making a repo, building, activating
 3 day to day         writing, editing, adding, and checking config
 4 modules            writing modules: lib.program, lib.service, config.nix
 5 formats            every output format and how nix values map to it
 6 packages           installing, removing, finding, updating, adopting
 7 source packages    templates of your own, drafted from nixpkgs or the aur
 8 services           runit services for the system and your session
 9 secrets            encrypted files, so the repo can be public
10 themes             colors from your wallpaper
11 machines           one repo for several machines
12 history            generations, rolling back, sharing
13 migrating          moving an existing setup into maw, step by step
14 troubleshooting    doctor, logs, and where to look
15 reference          where files go, and what maw keeps
```

Already have a configured machine? Read chapters 1 to 4, then follow chapter 13.

## Getting help

```sh
maw help              # list the chapters
maw help 4            # a chapter, by number
maw help modules      # or by name
maw help drift        # any section, by its heading
maw help add          # a command's options
```

Every command's `--help` ends with a `see:` line naming the section that explains it. On a terminal, help opens in `$PAGER` (`less` by default); set `NO_COLOR` to turn off styling.

## The TUI

```sh
maw
```

With no arguments, maw opens a full-screen view of everything it manages, one tab per area:

| tab | shows | keys |
|---|---|---|
| 1 packages | what's installed and declared, per source | `i` install, `r` remove, `f` find in the repos, `s` sync, `A` adopt |
| 2 modules | modules and static dirs, with a preview of the rendered file | `e`/enter edit, `n` new, `a` add a file |
| 3 services | declared and enabled services, with the selected one's log | `e` enable, `d` disable, `r` restart, `s` status |
| 4 status | the same report as `maw status` | `a` activate, `F` activate --force |
| 5 diff | the same diff as `maw diff` | `a` activate |
| 6 history | generations, newest first | `r`/enter roll back to the selected one |
| 7 git | the repo's status and recent commits | `c` commit, `p` push, `P` pull |

Everywhere: `j`/`k` or up/down move, `g`/`G` jump to the top or bottom, `h`/`l`, left/right, `1`-`7`, or tab switch tabs, `/` filters the list, `o` shows or hides the output pane, `R` reloads, `?` lists the keys, `q` quits. The mouse works too: click a tab or a row, scroll to move. On a narrow window the tab names shorten and previews move below the list.

Every action runs exactly what the matching command runs. Its output streams into a pane at the bottom, questions (a commit message, `[Y/n]`) appear as popups, and your editor takes over the screen until you close it. Removing, disabling, rolling back, and forcing ask for confirmation first. It asks for your sudo password when it opens, and again before an action if that has expired.

## Completions and man pages

The maw package installs tab completion for bash, zsh, and fish, and man pages: `man maw` for every command (`man maw-install`, `man maw-sv-enable`, ...), and each chapter of this manual as its own page, like `maw-getting-started(7)` or `maw-modules(5)`, the same text as `maw help`.

Completion knows your own names, not just commands: `maw edit <tab>` offers your modules, `maw remove <tab>` your declared packages, `maw sv restart <tab>` your services, `maw rollback <tab>` your generations with their messages. Running maw from a source checkout, load it yourself:

```sh
source <(COMPLETE=bash maw)                 # bash, in ~/.bashrc
source <(COMPLETE=zsh maw)                  # zsh, in ~/.zshrc
COMPLETE=fish maw | source                  # fish, in config.fish
```

Next: chapter 2, getting started (`maw help 2`).
