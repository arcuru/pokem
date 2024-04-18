{
  config,
  lib,
  pkgs,
  ...
}:
with lib; let
  cfg = config.services.pokem;
  yamlFormat = pkgs.formats.yaml {};
in {
  options.services.pokem = {
    enable = mkEnableOption "pokem service";
    package = mkOption {
      type = types.package;
      default = pkgs.pokem;
      example = literalExample "pkgs.pokem";
      description = "Package for the pokem service.";
    };
    settings = mkOption {
      type = yamlFormat.type;
      default = {};
      example = literalExpression ''
        {
            homeserver_url = "https://matrix.jackson.dev";
            username = "pokem";
            password = "hunter2";
            allow_list = "@me:matrix.org|@myfriend:matrix.org";
        }
      '';
      description = ''
        Configuration file for pokem. See the pokem documentation for more info.
      '';
    };
  };
  config = mkIf cfg.enable {
    systemd.user.services.pokem = {
      Unit = {
        Description = "Pokem Service";
        After = ["network-online.target"];
      };

      Service = {
        Environment = "RUST_LOG=error";
        ExecStart = "${cfg.package}/bin/pokem --daemon --config ${yamlFormat.generate "config.yml" (cfg.settings)}";
        Restart = "always";
      };

      Install.WantedBy = ["default.target"];
    };
  };
}
