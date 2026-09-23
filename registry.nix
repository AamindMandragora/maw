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
}
