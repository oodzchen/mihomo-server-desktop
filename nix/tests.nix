{ self, nixpkgs, pkgs, mihomo-server }:
let
  # Evaluated together with the service module, as a host would import both.
  system = nixpkgs.lib.nixosSystem {
    system = "x86_64-linux";
    modules = [
      mihomo-server.nixosModules.default
      self.nixosModules.default
      ({ ... }: {
        system.stateVersion = "26.05";
        boot.loader.grub.enable = false;
        fileSystems."/" = { device = "/dev/vda"; fsType = "ext4"; };
        users.users.alice.isNormalUser = true;
        services.mihomo-server = { enable = true; users = [ "alice" ]; };
        programs.mihomo-server-desktop.enable = true;
      })
    ];
  };
  cfg = system.config;
  package = cfg.programs.mihomo-server-desktop.package;
in
assert pkgs.lib.all (entry: entry.assertion) cfg.assertions;
assert package == self.packages.x86_64-linux.default;
assert builtins.elem package cfg.environment.systemPackages;
pkgs.runCommand "mihomo-server-desktop-nixos-module-check" {} ''
  echo ${pkgs.lib.escapeShellArg package.name} > $out
''
