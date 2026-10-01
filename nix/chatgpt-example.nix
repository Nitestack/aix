# Import alongside the aix Home Manager module. Install codex, opencode and/or pi
# separately and complete each tool's ChatGPT login (see chatgpt-example.toml).
{ ... }:
{
  programs.aix = {
    enable = true;
    profiles.chatgpt = {
      auth = "native";
      label = "ChatGPT subscription";
      # Choose a model supported by your ChatGPT plan.
      models.default = "gpt-5.4";
      tools = {
        codex.args = [
          "-c"
          ''model_provider="openai"''
          "-c"
          ''forced_login_method="chatgpt"''
          "--model"
          "{model}"
        ];
        opencode.args = [
          "--model"
          "openai/{model}"
        ];
        pi.args = [
          "--provider"
          "openai-codex"
          "--model"
          "{model}"
        ];
      };
      ask = {
        command = "pi";
        args = [
          "--print"
          "--mode"
          "text"
          "--no-session"
          "--no-tools"
          "--no-extensions"
          "--no-skills"
          "--no-prompt-templates"
          "--no-context-files"
          "--provider"
          "openai-codex"
          "--model"
          "{model}"
          "--system-prompt"
          "{system}"
        ];
      };
    };
  };
}
