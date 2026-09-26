# upstream dependency name (nixpkgs or arch) -> void package, for `maw src new --from-nix` and `--from-aur`
# null drops the dependency: nix-only build hooks, and tools a build_style already brings
# names that match a void package (or its -devel) as they are need no entry
{
  # nix-only helpers; anything named *-hook or *-hook.sh is dropped without an entry
  auditable-cargo = null;
  autoPatchelfHook = null;
  copyDesktopItems = null;
  install-shell-files = null;
  installShellFiles = null;
  makeBinaryWrapper = null;
  makeWrapper = null;
  nix-update-script = null;
  versionCheckHook = null;
  wrapGAppsHook3 = null;
  wrapGAppsHook4 = null;
  wrapQtAppsHook = null;
  writable-tmpdir-as-home-hook = null;
  writableTmpDirAsHomeHook = null;

  # arch's base system, which every void build already has
  gcc-libs = null;
  glibc = null;
  libgcc = null;

  # brought by the build_style
  cargo = null;
  cargo-auditable = null;
  cmake = null;
  go = null;
  meson = null;
  ninja = null;
  rustc = null;
  rustPlatform = null;

  # different names in void
  gtk3 = "gtk+3-devel";
  libGL = "libglvnd-devel";
  libpng-apng = "libpng-devel";
  pkg-config-wrapper = "pkg-config";
  resvg = "libresvg-devel";
  udev = "eudev-libudev-devel";
  wayland-scanner = "wayland-devel";
}
