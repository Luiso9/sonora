{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.sonora;
  json = pkgs.formats.json { };
  generated = json.generate "sonora-settings.json" ({ version = 1; } // cfg.settings);
  apply = pkgs.writeShellApplication {
    name = "sonora-apply-settings";
    runtimeInputs = [ pkgs.jq ];
    text = ''
      generated=$1
      dest="''${XDG_CONFIG_HOME:-$HOME/.config}/sonora/settings.json"
      mkdir -p "$(dirname "$dest")"
      if [[ -f "$dest" ]]; then
        jq --indent 2 -s '.[0] * .[1]' "$dest" "$generated" > "$dest.tmp"
        mv "$dest.tmp" "$dest"
      else
        cp "$generated" "$dest"
      fi
    '';
  };
  merge = "${lib.getExe apply} ${generated}";
  package =
    if cfg.settings == { } then
      cfg.package
    else
      pkgs.symlinkJoin {
        name = "sonora";
        paths = [ cfg.package ];
        nativeBuildInputs = [ pkgs.makeWrapper ];
        postBuild = ''
          wrapProgram $out/bin/sonora --run ${lib.escapeShellArg merge}
        '';
      };
in
{
  options.programs.sonora = {
    enable = lib.mkEnableOption "Sonora, a native music streaming client";

    package = lib.mkOption {
      type = lib.types.package;
    };

    settings = lib.mkOption {
      type = json.type;
      default = { };
      example = {
        provider = "youtube";
        gapless = true;
        appearance = {
          theme = "dark";
          icons = "solar";
        };
      };
      description = ''
        Configuration written to {file}`$XDG_CONFIG_HOME/sonora/settings.json`.
        Same JSON keys as the app. Set keys are merged on switch and again each
        launch; omitted keys, including resume and window bounds, are kept.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ package ];

    # write nix keys on switch; the wrapper merges them again on each launch
    home.activation.sonoraSettings = lib.mkIf (cfg.settings != { }) (
      lib.hm.dag.entryAfter [ "writeBoundary" ] ''
        $DRY_RUN_CMD ${merge}
      ''
    );
  };
}
