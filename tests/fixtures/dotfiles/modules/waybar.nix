{ config, lib, ... }:
let
  inherit (config) colors;

  # a collapsible group of modules
  drawer = modules: {
    orientation = "inherit";
    drawer.transition-duration = 250;
    inherit modules;
  };

  # the same properties on several selectors
  each = selectors: properties: { ${lib.concatStringsSep ",\n" selectors} = properties; };

  pills = [ "#clock" "#battery" "#cpu" "#memory" "#network" "#bluetooth" "#wireplumber" "#backlight" "#mpris" "#custom-off" ];
  hoverable = [ "#network" "#bluetooth" "#wireplumber" "#backlight" "#battery" "#cpu" "#memory" "#custom-off" ];
  sliders = [ "#pulseaudio-slider" "#backlight-slider" ];
  within = selectors: child: map (selector: "${selector} ${child}") selectors;

  white = alpha: "rgba(255, 255, 255, ${builtins.toJSON alpha})";
in
lib.program "waybar" {
  format = {
    main = "json";
    style = "css";
  };

  files.main = {
    layer = "top";
    position = "top";
    height = 34;
    spacing = 0;
    exclusive = true;

    modules-left = [ "group/device" "group/internet" "group/waves" ];
    modules-center = [ "clock" ];
    modules-right = [ "mpris" "niri/workspaces" "custom/off" ];

    clock = {
      tooltip = true;
      tooltip-format = "<tt>{calendar}</tt>";
      timezones = [ "" "Asia/Kolkata" ];
      format = "{0:%a %d %b  ·  %I:%M %p (%Z)}";
      actions.on-click = "tz_up";
      calendar = {
        mode = "month";
        weeks-pos = "left";
        format = {
          months = "<span color='#ffffff'><b>{}</b></span>";
          weeks = "<span color='#888888'>W{}</span>";
          weekdays = "<span color='#bbbbbb'><b>{}</b></span>";
          today = "<span color='#FF3B30'><b>{}</b></span>";
        };
      };
    };

    mpris = {
      format = "{status_icon}  <i>{title}</i> - {artist}";
      format-paused = "{status_icon}  <i>{title}</i> - {artist}";
      tooltip = false;
      status-icons = {
        playing = "<span font='8'>󰏤</span>";
        paused = "<span font='8'>󰐊</span>";
      };
      title-len = 20;
      artist-len = 12;
    };

    "niri/workspaces" = {
      format = "{icon}";
      format-icons = {
        focused = "●";
        active = "○";
        default = "•";
        empty = " ";
      };
      disable-click = false;
      all-outputs = false;
    };

    "custom/off" = {
      format = "⏻";
      tooltip = true;
      tooltip-format = "Power Off";
      on-click = config.powermenu;
    };

    "group/device" = drawer [ "battery" "cpu" "memory" ];
    "group/internet" = drawer [ "network" "bluetooth" ];
    "group/waves" = drawer [ "wireplumber" "pulseaudio/slider" "backlight" "backlight/slider" ];

    network = {
      format-wifi = "󰖩  {essid}";
      format-ethernet = "󰈀  {ifname}";
      format-disconnected = "󰖪";
      tooltip-format-wifi = "{signalStrength}%  ·  {ipaddr}  ·  {bandwidthUpBits}↑ {bandwidthDownBits}↓";
      on-click = "${config.terminal} -e 'sh -c \"sleep 0.005 && nmtui\"'";
    };

    bluetooth = {
      format = "󰂯  {status}";
      format-connected = "󰂱  {device_alias}";
      format-connected-battery = "󰂱  {device_alias}  {device_battery_percentage}%";
      format-disabled = "󰂲";
      format-off = "󰂲";
      tooltip-format-connected = "{device_enumerate}";
      tooltip-format-enumerate-connected = "{device_alias}\t{device_battery_percentage}%";
      on-click = "blueman-manager";
    };

    wireplumber = {
      format = "{icon}  {volume}%";
      format-muted = "󰝟  Muted";
      format-icons = [ "󰕿" "󰖀" "󰕾" ];
      on-click = "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle";
      tooltip-format = "Volume: {volume}%\nNode: {node_name}";
    };

    "pulseaudio/slider" = {
      min = 0;
      max = 100;
      orientation = "horizontal";
      on-click = "pwvucontrol";
    };

    backlight = {
      format = "{icon}  {percent}%";
      format-icons = [ "󰃞" "󰃟" "󰃠" ];
      tooltip-format = "Brightness: {percent}%";
    };

    "backlight/slider" = {
      min = 5;
      max = 100;
      orientation = "horizontal";
    };

    battery = {
      bat = "BAT0";
      interval = 30;
      states = {
        warning = 25;
        critical = 10;
      };
      format = "{icon}  {capacity}%";
      format-charging = "󰂄  {capacity}%";
      format-full = "󰁹  Full";
      format-icons = [ "󰁺" "󰁻" "󰁼" "󰁽" "󰁾" "󰁿" "󰂀" "󰂁" "󰂂" "󰁹" ];
      tooltip-format = "{timeTo}\n{power:.1f}W draw";
    };

    cpu = {
      interval = 1;
      format = "{usage:>2}% cpu";
    };

    memory = {
      interval = 30;
      format = "{percentage}% memory";
    };
  };

  # a list, because later rules must override earlier ones
  files.style = [
    {
      "*" = {
        font-family = [ ''"${config.font.ui}"'' ''"${config.font.icons}"'' "sans-serif" ];
        font-size = "13px";
        font-weight = 500;
        min-height = 0;
        margin = 0;
        padding = "0px 3px";
      };

      "window#waybar" = {
        background = "rgba(0, 0, 0, 0.9)";
        padding = "2px 8px";
        color = "#${colors.text}";
      };

      button = {
        border = "none";
        box-shadow = "none";
        background = "transparent";
      };

      tooltip = {
        background = "rgba(28, 28, 30, 0.92)";
        border = "1px solid ${white 0.1}";
        color = "#${colors.text}";
        padding = "6px 10px";
        font-size = "12px";
        label.color = "#${colors.text}";
      };
    }

    (each pills {
      font-size = "12px";
      font-weight = 500;
      padding = "4px 9px";
      margin = "4px 0px";
      color = "#${colors.dim}";
      transition = [ "background 0.15s ease" "color 0.15s ease" ];
    })

    {
      "#clock" = {
        color = "#${colors.text}";
        font-weight = 600;
        font-size = "13px";
        letter-spacing = "0.2px";
        padding = "4px 11px";
      };

      "#workspaces" = {
        margin = "0 6px";
        button = {
          color = white 0.25;
          padding = "4px 8px";
          margin = "4px 1px";
          font-size = "10px";
          transition = "all 0.15s ease";
          "&.active" = {
            color = "#${colors.text}";
            background = white 0.14;
          };
          "&:hover" = {
            color = "#${colors.text}";
            background = white 0.07;
          };
        };
      };

      "#mpris.playing".color = "#${colors.text}";
    }

    (each [ "#network.disconnected" "#bluetooth.off" "#bluetooth.disabled" "#wireplumber.muted" ] {
      color = white 0.28;
    })

    {
      "#battery" = {
        "&.charging".color = "#7fd88f";
        "&.warning:not(.charging)".color = "#f5c76a";
        "&.critical:not(.charging)" = {
          color = "#ff6961";
          animation = "pulse 1.5s ease-in-out infinite";
        };
      };

      "@keyframes pulse" = {
        "0%".opacity = 1;
        "50%".opacity = 0.5;
        "100%".opacity = 1;
      };
    }

    (each [ "#group-device" "#group-internet" "#group-waves" ] { padding = 0; })

    (each sliders {
      padding = "0 4px";
      margin = "4px 0";
    })

    (each (within sliders "trough") {
      min-height = "6px";
      min-width = "72px";
      background = white 0.09;
    })

    (each (within sliders "highlight") {
      min-width = "6px";
      border-radius = "3px";
      background = white 0.5;
    })

    (each (within sliders "slider") {
      opacity = 0;
      min-height = 0;
      min-width = 0;
      background-image = "none";
      border = "none";
      box-shadow = "none";
    })

    (each (map (selector: "${selector}:hover") hoverable) {
      background = white 0.07;
      color = "#${colors.text}";
    })

    {
      "#calendar" = {
        color = "#${colors.muted}";
        font-size = "12px";
        "&.header" = {
          color = "#${colors.text}";
          font-weight = 600;
        };
      };
    }
  ];
}
