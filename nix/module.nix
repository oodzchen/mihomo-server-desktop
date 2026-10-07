{ self }:
{ config, lib, pkgs, ... }:
let
  cfg = config.programs.mihomo-server-desktop;
in {
  options.programs.mihomo-server-desktop = {
    enable = lib.mkEnableOption "Mihomo Server desktop client";
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      description = "Desktop client package.";
    };
  };
  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
  };
}
