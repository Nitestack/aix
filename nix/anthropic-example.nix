# Import alongside the aix Home Manager module. Install Claude Code separately
# and use `aix claude anthropic -- auth login` to sign in with your Claude account.
{ ... }:
{
  programs.aix = {
    enable = true;
    profiles.anthropic = {
      auth = "native";
      label = "Claude subscription";
      models.default = "sonnet";
      tools.claude.args = [
        "--model"
        "{model}"
        "--settings"
        ''{"forceLoginMethod":"claudeai"}''
      ];
      ask = {
        command = "claude";
        args = [
          "--print"
          "--output-format"
          "text"
          "--no-session-persistence"
          "--safe-mode"
          "--tools"
          ""
          "--disable-slash-commands"
          "--strict-mcp-config"
          "--mcp-config"
          ''{"mcpServers":{}}''
          "--setting-sources"
          ""
          "--settings"
          ''{"forceLoginMethod":"claudeai"}''
          "--model"
          "{model}"
          "--system-prompt"
          "{system}"
        ];
      };
    };
  };
}
