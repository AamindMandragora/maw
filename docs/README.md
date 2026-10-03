# The maw manual

maw manages a Void Linux machine from one git repo: packages, runit services, and config files. You describe config in Nix, and maw renders it to each program's native format and puts it in place.

Read it in order the first time; each chapter ends by pointing at the next. The same text is in `maw help` (`maw help 3` opens chapter 3) and the man pages.

1. [Introduction](manual/01-introduction.md): what maw is, getting help, the TUI
2. [Getting started](manual/02-getting-started.md): making a repo, building, activating
3. [Day to day](manual/03-day-to-day.md): writing, editing, adding, and checking config
4. [Modules](manual/04-modules.md): writing modules: `lib.program`, `lib.service`, `config.nix`
5. [Formats](manual/05-formats.md): every output format and how Nix values map to it
6. [Packages](manual/06-packages.md): installing, removing, finding, updating, adopting
7. [Source packages](manual/07-source-packages.md): templates of your own, drafted from nixpkgs or the AUR
8. [Services](manual/08-services.md): runit services for the system and your session
9. [Secrets](manual/09-secrets.md): encrypted files, so the repo can be public
10. [Themes](manual/10-themes.md): colors from your wallpaper
11. [Machines](manual/11-machines.md): one repo for several machines
12. [History](manual/12-history.md): generations, rolling back, sharing
13. [Migrating](manual/13-migrating.md): moving an existing setup into maw, step by step
14. [Troubleshooting](manual/14-troubleshooting.md): doctor, logs, and where to look
15. [Reference](manual/15-reference.md): where files go, and what maw keeps

Contributors: [development.md](development.md) covers the layout, tests, and releasing.
