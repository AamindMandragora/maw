# program name -> where its files go; format is the default for new modules
# paths are under ~/.config unless they start with ~/ (home) or / (system root)
{
  # one destination per file role
  alacritty = {
    format = "toml";
    files.main = "alacritty/alacritty.toml";
  };
  bash = {
    format = "shell";
    files = {
      main = "~/.bashrc";
      profile = "~/.bash_profile";
    };
  };
  # roles not listed in files go under dir, which defaults to the program name
  fonts.dir = "~/.local/share/fonts";
  foot = {
    format = "ini";
    files.main = "foot/foot.ini";
  };
  fuzzel = {
    format = "ini";
    files.main = "fuzzel/fuzzel.ini";
  };
  ghostty = {
    format = "keyValue";
    files.main = "ghostty/config";
  };
  git = {
    format = "ini";
    files.main = "git/config";
  };
  # copied as root instead of symlinked
  greetd = {
    format = "toml";
    files.main = "/etc/greetd/config.toml";
    dir = "/etc/greetd";
    root = true;
  };
  mako = {
    format = "ini";
    files.main = "mako/config";
  };
  niri = {
    format = "kdl";
    files.main = "niri/config.kdl";
  };
  # every file is made executable
  scripts = {
    dir = "~/.local/bin";
    executable = true;
  };
  swaync = {
    format = "json";
    files = {
      main = "swaync/config.json";
      style = "swaync/style.css";
    };
  };
  tmux = {
    format = "raw";
    files.main = "tmux/tmux.conf";
  };
  wallpapers.dir = "~/.local/share/wallpapers";
  waybar = {
    format = "json";
    files = {
      main = "waybar/config.jsonc";
      style = "waybar/style.css";
    };
  };
  # from home-manager by tools/scrape-registry; edit or move freely
  abook = {
    files.main = "abook/abookrc";
  };
  afew = {
    files.main = "afew/config";
  };
  ahoviewer = {
    files.main = "ahoviewer/ahoviewer.cfg";
  };
  alot = {
    files = {
      main = "alot/config";
      hooks = "alot/hooks.py";
    };
  };
  amfora = {
    format = "toml";
    files.main = "amfora/config.toml";
  };
  aria2 = {
    files.main = "aria2/aria2.conf";
  };
  asciinema = {
    format = "toml";
    files.main = "asciinema/config.toml";
  };
  astroid = {
    files = {
      main = "astroid/config";
      poll = "astroid/poll.sh";
    };
  };
  atool = {
    files.main = "~/.atoolrc";
  };
  bashmount = {
    files.main = "bashmount/config";
  };
  beets = {
    files.main = "beets/config.yaml";
  };
  bluetuith = {
    files.main = "bluetuith/bluetuith.conf";
  };
  bottom = {
    format = "toml";
    files.main = "bottom/bottom.toml";
  };
  cava = {
    files.main = "cava/config";
  };
  cmus = {
    files.main = "~/.config/cmus/rc";
  };
  darcs = {
    files = {
      main = "~/.darcs/author";
      boring = "~/.darcs/boring";
      defaults = "~/.darcs/defaults";
    };
  };
  eilmeldung = {
    format = "toml";
    files.main = "eilmeldung/config.toml";
  };
  eza = {
    files.main = "eza/theme.yml";
  };
  fastfetch = {
    format = "json";
    files.main = "fastfetch/config.jsonc";
  };
  fd = {
    files.main = "fd/ignore";
  };
  feh = {
    files = {
      main = "feh/buttons";
      keys = "feh/keys";
      themes = "feh/themes";
    };
  };
  gallery-dl = {
    format = "json";
    files.main = "gallery-dl/config.json";
  };
  gitui = {
    files = {
      main = "gitui/theme.ron";
      key_bindings = "gitui/key_bindings.ron";
    };
  };
  go = {
    files = {
      main = "go/env";
      mode = "go/telemetry/mode";
    };
  };
  grype = {
    files.main = "grype/config.yaml";
  };
  havoc = {
    files.main = "havoc.cfg";
  };
  himalaya = {
    format = "toml";
    files.main = "himalaya/config.toml";
  };
  htop = {
    files.main = "htop";
  };
  i3bar-river = {
    format = "toml";
    files.main = "i3bar-river/config.toml";
  };
  i3status = {
    files.main = "i3status/config";
  };
  impala = {
    format = "toml";
    files.main = "impala/config.toml";
  };
  ion = {
    files.main = "ion/initrc";
  };
  irssi = {
    files.main = "~/.irssi/config";
  };
  jrnl = {
    files.main = "jrnl/jrnl.yaml";
  };
  keepassxc = {
    format = "ini";
    files.main = "keepassxc/keepassxc.ini";
  };
  khal = {
    files.main = "khal/config";
  };
  khard = {
    files.main = "khard/khard.conf";
  };
  kitty = {
    files = {
      main = "kitty/kitty.conf";
      quick-access-terminal = "kitty/quick-access-terminal.conf";
      diff = "kitty/diff.conf";
      "light-theme.auto" = "kitty/light-theme.auto.conf";
      "dark-theme.auto" = "kitty/dark-theme.auto.conf";
      "no-preference-theme.auto" = "kitty/no-preference-theme.auto.conf";
      macos-launch-services-cmdline = "kitty/macos-launch-services-cmdline";
    };
  };
  lazysql = {
    format = "toml";
    files.main = "lazysql/config.toml";
  };
  ledger = {
    files.main = "ledger/ledgerrc";
  };
  less = {
    files.main = "lesskey";
  };
  lf = {
    files.main = "lf/lfrc";
  };
  meli = {
    format = "toml";
    files.main = "meli/config.toml";
  };
  mercurial = {
    files = {
      main = "hg/hgrc";
      hgignore_global = "hg/hgignore_global";
    };
  };
  micro = {
    format = "json";
    files.main = "micro/settings.json";
  };
  mpv = {
    files = {
      main = "mpv/mpv.conf";
      input = "mpv/input.conf";
    };
  };
  mpvpaper = {
    files = {
      main = "mpvpaper/pauselist";
      stoplist = "mpvpaper/stoplist";
    };
  };
  msmtp = {
    files.main = "msmtp/config";
  };
  ncspot = {
    format = "toml";
    files.main = "ncspot/config.toml";
  };
  neomutt = {
    files.main = "neomutt/neomuttrc";
  };
  neovide = {
    format = "toml";
    files.main = "neovide/config.toml";
  };
  nnn = {
    files.main = "nnn/plugins";
  };
  nyxt = {
    files.main = "nyxt/config.lisp";
  };
  offlineimap = {
    files = {
      main = "offlineimap/config";
      get_settings = "offlineimap/get_settings.py";
      get_settings2 = "offlineimap/get_settings.pyc";
    };
  };
  papis = {
    files.main = "papis/config";
  };
  pgcli = {
    files.main = "pgcli/config";
  };
  pianobar = {
    files.main = "pianobar/config";
  };
  pimsync = {
    files.main = "pimsync/pimsync.conf";
  };
  pqiv = {
    files.main = "pqivrc";
  };
  pylint = {
    files.main = "~/.pylintrc";
  };
  pyradio = {
    files = {
      main = "pyradio/config";
      stations = "pyradio/stations.csv";
    };
  };
  qalculate = {
    files.main = "qalculate/qalc.cfg";
  };
  ranger = {
    files = {
      main = "ranger/rc.conf";
      rifle = "ranger/rifle.conf";
    };
  };
  rbenv = {
    files.main = "~/.rbenv/plugins";
  };
  readline = {
    files.main = "~/.inputrc";
  };
  retext = {
    files.main = "ReText Project/ReText.conf";
  };
  rio = {
    format = "toml";
    files.main = "rio/config.toml";
  };
  rtorrent = {
    files.main = "rtorrent/rtorrent.rc";
  };
  ruff = {
    format = "toml";
    files.main = "ruff/ruff.toml";
  };
  satty = {
    format = "toml";
    files.main = "satty/config.toml";
  };
  screen = {
    files.main = "~/.screenrc";
  };
  sheldon = {
    format = "toml";
    files.main = "sheldon/plugins.toml";
  };
  sioyek = {
    files = {
      main = "sioyek/prefs_user.config";
      keys_user = "sioyek/keys_user.config";
    };
  };
  stylua = {
    format = "toml";
    files.main = "stylua/stylua.toml";
  };
  swappy = {
    files.main = "swappy/config";
  };
  swayimg = {
    files.main = "swayimg/init.lua";
  };
  swaylock = {
    files.main = "swaylock/config";
  };
  swayr = {
    format = "toml";
    files.main = "swayr/config.toml";
  };
  terminator = {
    files.main = "terminator/config";
  };
  tmate = {
    files.main = "~/.tmate.conf";
  };
  todoman = {
    files.main = "todoman/config.py";
  };
  tofi = {
    files.main = "tofi/config";
  };
  topgrade = {
    format = "toml";
    files.main = "topgrade.toml";
  };
  translate-shell = {
    files.main = "translate-shell/init.trans";
  };
  trippy = {
    format = "toml";
    files.main = "trippy/trippy.toml";
  };
  ty = {
    format = "toml";
    files.main = "ty/ty.toml";
  };
  uv = {
    format = "toml";
    files.main = "uv/uv.toml";
  };
  vdirsyncer = {
    files.main = "vdirsyncer/config";
  };
  vifm = {
    files.main = "vifm/vifmrc";
  };
  vinegar = {
    format = "toml";
    files.main = "vinegar/config.toml";
  };
  visidata = {
    files.main = "~/.visidatarc";
  };
  wiremix = {
    format = "toml";
    files.main = "wiremix/wiremix.toml";
  };
  wleave = {
    format = "json";
    files = {
      main = "wleave/layout.json";
      style = "wleave/style.css";
    };
  };
  wlogout = {
    files = {
      main = "wlogout/layout";
      style = "wlogout/style.css";
    };
  };
  xmobar = {
    files.main = "xmobar/.xmobarrc";
  };
  yambar = {
    files.main = "yambar/config.yml";
  };
  yt-dlp = {
    files.main = "yt-dlp/config";
  };
  zathura = {
    files.main = "zathura/zathurarc";
  };
  zk = {
    format = "toml";
    files.main = "zk/config.toml";
  };
  zsh = {
    files.main = "~/.zshenv";
  };
}
