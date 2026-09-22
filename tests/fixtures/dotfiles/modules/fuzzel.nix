{ config, lib, ... }:
lib.program "fuzzel" {
  format = "ini";
  settings = {
    main = {
      font = "${config.font.ui}:size=16";
      prompt = ''"🔍 "'';
      width = 48;
      lines = 8;
      line-height = 28;
      horizontal-pad = 24;
      vertical-pad = 18;
      inner-pad = 10;
      anchor = "center";
      y-margin = 180;
      layer = "overlay";
      icons-enabled = "yes";
      image-size-ratio = 0.4;
      fields = lib.concatStringsSep "," [ "filename" "name" "generic" "keywords" ];
    };

    border = {
      radius = 30;
      width = 2;
      selection-radius = 15;
    };

    colors = with config.colors; {
      background = "1c1c1eE6";
      text = "${text}ff";
      placeholder = "${muted}ff";
      selection = "ffffff1f";
      selection-text = "ffffffff";
      selection-match = "ffffffff";
      border = "ffffff1a";
      match = "${muted}ff";
    };
  };
}
