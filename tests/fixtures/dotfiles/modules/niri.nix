{ config, lib, ... }:
let
  inherit (lib.kdl) node;

  # a node with only properties, e.g. `match app-id="firefox"`
  props = attrs: node [ ] attrs { };

  # one bind per workspace number
  workspaceBinds = prefix: action: lib.listToAttrs (map (n: lib.nameValuePair "${prefix}${toString n}" { ${action} = n; }) (lib.range 1 9));

  wpctl = args: { spawn-sh = "wpctl ${args}"; };
  playerctl = args: { spawn-sh = "playerctl ${args}"; };
  sh = command: { spawn = [ "sh" "-c" command ]; };
in
lib.program "niri" {
  format = "kdl";
  settings = {
    input = {
      keyboard = {
        xkb.options = "compose:ralt";
        numlock = null;
      };
      touchpad = {
        tap = null;
        dwt = null;
        dwtp = null;
        natural-scroll = null;
        accel-speed = 0.5;
        accel-profile = "adaptive";
      };
      mouse = {
        accel-speed = 1;
        accel-profile = "adaptive";
      };
      tablet.map-to-output = "DP-1";
      disable-power-key-handling = null;
    };

    layer-rule = {
      match = props { namespace = "^wallpaper$"; };
      place-within-backdrop = true;
    };

    layout = {
      gaps = 0;
      center-focused-column = "never";
      background-color = "transparent";
      preset-column-widths.proportion = map (width: [ width ]) [ 0.25 0.33333 0.5 0.66667 0.75 1.0 ];
      default-column-width.proportion = 0.5;
      default-column-display = "tabbed";
      focus-ring.off = null;
      border.off = null;
      tab-indicator = {
        position = "left";
        place-within-column = null;
        hide-when-single-tab = null;
        width = 6;
        gap = 4;
        length = props { total-proportion = 1.0; };
        corner-radius = 16;
      };
    };

    overview = {
      zoom = 0.65;
      workspace-shadow.off = null;
    };

    recent-windows.binds = { };
    gestures.hot-corners.off = null;

    output = [
      (node [ "eDP-1" ] { } {
        position = props { x = 0; y = 0; };
        scale = 2;
      })
      (node [ "DP-4" ] { } {
        mode = "2560x1440@59.951";
        position = props { x = 1440; y = -540; };
      })
    ];

    environment = {
      QT_QPA_PLATFORM = "wayland";
      QT_QPA_PLATFORMTHEME = "qt5ct";
    };

    spawn-at-startup = [
      [ "gnome-keyring-daemon" "--start" "--components=pkcs11,secrets,ssh" ]
      [ "wl-paste" "--watch" "cliphist" "store" ]
      [ "swaybg" "-i" config.wallpaper ]
      [ "waybar" ]
      [ "gsettings" "set" "org.gnome.desktop.interface" "color-scheme" "prefer-dark" ]
      [ "pipewire" ]
      [ "sh" "-c" "sleep 0.5 && pipewire-pulse" ]
      [ "sh" "-c" "sleep 1 && wireplumber" ]
    ];

    hotkey-overlay.skip-at-startup = null;
    prefer-no-csd = null;
    screenshot-path = "~/Pictures/Screenshots/%Y-%m-%d_%H-%M-%S.png";

    window-rule = [
      {
        match = props { app-id = ''^org\.wezfurlong\.wezterm$''; };
        default-column-width = { };
      }
      {
        geometry-corner-radius = 0;
        clip-to-geometry = true;
      }
      {
        match = props { app-id = ''^ssf2\.exe''; };
        open-maximized = true;
        min-width = 1424;
        min-height = 844;
      }
      {
        match = props { app-id = "[Rr]yujinx"; title = "ContentDialogOverlayWindow"; };
        open-floating = true;
      }
    ];

    binds = workspaceBinds "Mod+" "focus-workspace" // workspaceBinds "Mod+Ctrl+" "move-column-to-workspace" // {
      "Mod+Shift+Slash".show-hotkey-overlay = null;
      "Mod+T" = node [ ] { hotkey-overlay-title = config.terminal; } { spawn = config.terminal; };
      "Mod+Space" = node [ ] { hotkey-overlay-title = "fuzzel"; } { spawn = "fuzzel"; };

      XF86AudioRaiseVolume = wpctl "set-volume @DEFAULT_AUDIO_SINK@ 0.05+ -l 1.0";
      XF86AudioLowerVolume = wpctl "set-volume @DEFAULT_AUDIO_SINK@ 0.05-";
      XF86AudioMute = wpctl "set-mute @DEFAULT_AUDIO_SINK@ toggle";
      XF86AudioMicMute = wpctl "set-mute @DEFAULT_AUDIO_SOURCE@ toggle";
      "Mod+TouchpadScrollDown" = wpctl "set-volume @DEFAULT_AUDIO_SINK@ 0.02+ -l 1.0";
      "Mod+TouchpadScrollUp" = wpctl "set-volume @DEFAULT_AUDIO_SINK@ 0.02-";

      XF86AudioPlay = playerctl "play-pause";
      XF86AudioStop = playerctl "stop";
      XF86AudioPrev = playerctl "previous";
      XF86AudioNext = playerctl "next";

      XF86MonBrightnessUp.spawn = [ "brightnessctl" "--class=backlight" "set" "+10%" ];
      XF86MonBrightnessDown.spawn = [ "brightnessctl" "--class=backlight" "set" "10%-" ];

      XF86PowerOff.spawn = config.powermenu;
      XF86WakeUp.spawn = "true";

      "Mod+O" = node [ ] { repeat = false; } { toggle-overview = null; };
      "Mod+Q" = node [ ] { repeat = false; } { close-window = null; };

      "Mod+Left".focus-column-or-monitor-left = null;
      "Mod+Right".focus-column-or-monitor-right = null;
      "Mod+Up".focus-window-or-workspace-up = null;
      "Mod+Down".focus-window-or-workspace-down = null;

      "Mod+Ctrl+Left".move-column-left = null;
      "Mod+Ctrl+Down".move-window-down = null;
      "Mod+Ctrl+Up".move-window-up = null;
      "Mod+Ctrl+Right".move-column-right = null;

      "Mod+Shift+Left".move-column-to-monitor-left = null;
      "Mod+Shift+Down".move-column-to-monitor-down = null;
      "Mod+Shift+Up".move-column-to-monitor-up = null;
      "Mod+Shift+Right".move-column-to-monitor-right = null;

      "Mod+WheelScrollDown" = node [ ] { cooldown-ms = 200; } { focus-window-or-workspace-down = null; };
      "Mod+WheelScrollUp" = node [ ] { cooldown-ms = 200; } { focus-window-or-workspace-up = null; };
      "Mod+Shift+WheelScrollDown".focus-column-or-monitor-right = null;
      "Mod+Shift+WheelScrollUp".focus-column-or-monitor-left = null;

      "Mod+Tab".focus-workspace-previous = null;
      "Mod+BracketLeft".consume-or-expel-window-left = null;
      "Mod+BracketRight".consume-or-expel-window-right = null;

      "Mod+R".switch-preset-column-width = null;
      "Mod+Ctrl+Shift+R".switch-preset-window-height = null;
      "Mod+Ctrl+R".reset-window-height = null;

      "Mod+F".maximize-column = null;
      "Mod+Shift+F".fullscreen-window = null;
      "Mod+C".center-column = null;

      "Mod+Minus".set-column-width = "-10%";
      "Mod+Equal".set-column-width = "+10%";
      "Mod+Shift+Minus".set-window-height = "-10%";
      "Mod+Shift+Equal".set-window-height = "+10%";

      "Mod+A".toggle-window-floating = null;
      "Mod+Shift+A".switch-focus-between-floating-and-tiling = null;
      "Mod+W".toggle-column-tabbed-display = null;

      "Mod+S".screenshot = null;
      "Mod+Ctrl+S".screenshot-screen = null;
      "Mod+Shift+S".screenshot-window = null;

      "Mod+E".spawn = "nautilus";
      "Mod+V".spawn = "vivaldi-stable";
      "Mod+Period".spawn = "emote";

      "Mod+Alt+S".spawn = [ "bash" "-c" ''grim -g "$(slurp)" - | tesseract stdin stdout -l eng 2>/dev/null | wl-copy'' ];
      "Mod+Shift+R" = sh ''GEOM=$(slurp) && REGION=$(echo "$GEOM" | awk -F'[ ,x]' '{print $3"x"$4"+"$1"+"$2}') && gpu-screen-recorder -encoder cpu -w region -region "$REGION" -a default_output -f 60 -o ~/Videos/Captures/$(date +%Y-%m-%d_%H-%M-%S).mp4'';
      "Mod+Alt+R" = sh "pkill gpu-screen-recorder";
      "Mod+L" = sh "cliphist list | fuzzel --dmenu | cliphist decode | wl-copy";

      "Ctrl+Alt+Delete".quit = null;
    };
  };
}
