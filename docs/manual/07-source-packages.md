# 7. Source packages

Anything the Void repos don't have, you can build yourself with an xbps-src template in `srcpkgs/<name>/template`:

```sh
maw src new hello        # writes a blank template and opens it
maw install hello        # builds it with xbps-src, installs it, records it
maw src build hello      # rebuilds after you change the template
```

To draft a template from nixpkgs instead of writing one:

```sh
maw src new lazygit --from-nix           # from nixpkgs' lazygit
maw src new rg --from-nix ripgrep        # a different name than nixpkgs uses
maw src update lazygit                   # later: move it to nixpkgs' current version, then rebuild
```

maw reads the package's metadata from nixpkgs (it never builds with nix) and fills in the version, description, license, homepage, source url and its checksum, the build style (go, cargo, meson, cmake, python, or configure), and the dependencies, using Void's names:

```
# scaffolded by maw from nixpkgs 'ripgrep' at 4975466d3247
pkgname=ripgrep
version=15.2.0
revision=1
build_style=cargo
hostmakedepends="pkg-config"
makedepends="pcre2-devel"
# TODO: nix had 'libfoo'
...
```

A dependency maw can't match to a Void package is left as a `# TODO` line. Fix it by hand; when you close the editor, maw asks whether to remember what you replaced it with (`record libfoo -> foo-devel in depmap? [Y/n]`) and saves it to `depmap.nix` in your repo, so the next scaffold gets it right. Nix-only build helpers are dropped on their own. Check a draft before building: nixpkgs sometimes patches or configures a package in ways a template needs spelled out, like ripgrep's `configure_args="--features=pcre2"`.

The nixpkgs checkout lives at `~/.local/share/maw/nixpkgs` (a shallow nixos-unstable clone, about 400MB), made on first use; `maw.nixpkgs` in `config.nix` points elsewhere.

Or from an AUR package, which works the same way:

```sh
maw src new wlogout --from-aur           # from the aur's wlogout
maw src new yay --from-aur yay-bin       # a different name than the aur uses
```

maw reads the AUR's metadata and the package's PKGBUILD. It never runs the PKGBUILD: it reads its plain assignments and simple `${var}` expansions, so anything cleverer shows up as a `# TODO`. The build style comes from the build tools and the PKGBUILD's `build()`. Arch's `depends` holds both libraries and programs, so libraries go to `makedepends` as their `-devel` package and programs to `depends`. The PKGBUILD's `package()` is kept as comments at the end of the draft, for install steps the build style doesn't cover (licenses, completions, config files):

```
# the PKGBUILD's package(), for anything the build style doesn't cover:
#	install -Dm644 man/paru.8 "$pkgdir/usr/share/man/man8/paru.8"
#	install -Dm644 completions/zsh "${pkgdir}/usr/share/zsh/site-functions/_paru"
```

A `-bin` package that repackages prebuilt files gets no build style and a TODO to port its `package()` into `do_install()`. A `-git` package builds from a checkout, which xbps-src can't do; draft from the release package instead. Arch-only dependencies, like `pacman`, stay as TODOs to delete.

`src update` works on templates drafted either way, which it recognizes by their `# scaffolded by maw` line; it touches only `version`, `revision`, `distfiles`, and `checksum`, so your other edits stay. On a template you wrote yourself, it follows the upstream's releases instead; see [updating](06-packages.md#updating).

### Patching Void's packages

To change a package Void already has, give it patches instead of a template:

```
srcpkgs/gnome-network-displays/
  patches/
    gnd-dmabuf-gl.patch
```

```sh
maw src build gnome-network-displays    # void's template, plus your patches
```

maw builds Void's own template with your patches applied after Void's (in name order, or the order of a `series` file you put next to them), installs it in place of Void's build, and holds it so `maw sync` never swaps Void's unpatched build back. When Void releases a new version, `maw status` shows `outdated gnome-network-displays 0.99.0_1 -> 1.0.0_1 + patches`, and `maw src build` (or the next `maw sync`) updates the clone to Void's latest templates and rebuilds with your patches. If a patch stops applying to the new version, the build fails and the patched old version stays installed. Delete the `srcpkgs/` dir to go back to Void's build; the next `maw sync` releases the hold and reinstalls it.

A name with a template in `srcpkgs/` is always a source build, and it's recorded under `packages.xbps` like any other package; the template's presence is what makes it one. The template is written in xbps-src's own format; see the [Void manual on templates](https://github.com/void-linux/void-packages/blob/master/Manual.md).

Builds happen in a void-packages clone maw keeps at `~/.local/share/maw/void-packages`. The first build clones it (shallowly) and bootstraps its build root, which takes a few minutes; after that, each template is copied into the clone's `srcpkgs/` and built with `xbps-src pkg <name>`, and the package is installed from the clone's `hostdir/binpkgs`. A template can't use the name of a package void-packages already has; patch that package instead. To build in a clone of your own instead, in `config.nix`:

```nix
maw.voidPackages = "~/void-packages";
```

Activation builds and installs a declared source package that isn't installed at all. It doesn't rebuild on its own when you change a template: `maw status` shows `outdated hello 0.1_1 -> 0.2_1`, and `maw src build hello` (or the next `maw sync`) rebuilds it and upgrades the installed package.

While any source package is declared, maw also manages `/etc/xbps.d/10-maw-local.conf`, which adds the clone's `hostdir/binpkgs` to xbps's repositories, so `xbps-query`, `xbps-install`, and `xbps-install -Su` see your builds like any other package.

### Cleaning up

Building leaves things behind in the clone: dependencies xbps-src downloaded for the build, and every version of every package it built. After each source package it installs, maw cleans up: the dependencies downloaded to build with go (the next build downloads what it needs again), and so does every built version that's neither installed nor recorded in a [generation](12-history.md), since rollback installs those from here. [Squashing](12-history.md#squashing) generations lets old builds go too. To do the same by hand:

```sh
maw src clean    # freed 612 MB
```

Next: chapter 8, services (`maw help 8`).
