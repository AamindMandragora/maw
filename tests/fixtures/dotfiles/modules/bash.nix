{ config, lib, ... }:
lib.program "bash" {
  format = "shell";
  settings = {
    aliases = {
      ll = "ls -l";
      gs = "git status";
    };
    exports.EDITOR = "nvim";
    exports.PATH = lib.raw "\"$HOME/.local/bin:$PATH\"";
    extra = lib.raw ''
      PS1='\w \$ '
    '';
  };
}
