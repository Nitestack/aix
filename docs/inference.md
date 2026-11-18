# One-shot inference

Model names and aliases resolve through the [model configuration](configuration.md#model-defaults-and-aliases).

`aix ask` sends one instruction and any explicitly supplied text context to the selected profile's OpenAI-compatible `/v1/chat/completions` endpoint. `aix prompt NAME` sends the configured preset instruction, optional system message, and explicit context through the same inference pipeline. Neither command caches prompts or responses.

```sh
# Diagnose a service log; stdin becomes explicit context.
journalctl -u nginx -n 100 | aix ask "Identify the likely failure and suggest the next diagnostic step"

# Summarize a document from a pipe.
cat contract.txt | aix ask "Summarize the obligations and deadlines"

# Add one or more explicitly named files as context.
aix ask --file notes.md "Turn these notes into a concise summary"

# Select a profile and model alias.
aix --profile work ask --model smart "Explain TCP slow start"
```

The optional `--system TEXT` is sent as the only system message. Repeated `--file PATH` values are sent after piped stdin, in command-line order, with file boundaries that include each supplied path. If there is no instruction, non-empty piped stdin becomes the user message; file context alone requires an instruction. `aix ask` does not start an interactive chat.

Input to `ask` and `prompt` is sent to the configured gateway. Only the instruction, non-TTY stdin, and paths passed through `--file` are read. **aix does not inspect the current project or coding harness, inspect Git state, discover neighboring files, or infer project context.** These are domain-agnostic inference commands; pipes and files are ordinary user-supplied context.

Human output contains only the assistant's text and a trailing newline. Use `--json` for the stable aix JSON envelope, which includes the resolved model, assistant content, and token usage (null when the gateway omits a field).

## Reusable prompt presets

Define named instructions under `[prompts.<name>]` to reuse them with `aix prompt`:

```toml
[prompts.summarize]
prompt = "Summarize the supplied material clearly and concisely."

[prompts.diagnose]
prompt = "Analyze the supplied diagnostic output and suggest the next verification step."
system = "Be precise and distinguish evidence from inference."
model = "smart"

[prompts.extract-actions]
prompt = "Extract concrete actions, owners, dates, and unresolved questions from the supplied notes."

# An optional user-defined code-oriented preset is ordinary prompt text too.
[prompts.review-code]
prompt = "Review the supplied change for correctness, regressions, and security issues."
```

`prompt` is required and non-empty. `system` and `model` are optional and must
be non-empty when present. A model can be an alias or raw model ID. Selection
uses `--model`, then the preset's model, then the selected profile's default,
then the global default. Aliases resolve through the same model configuration
as `aix ask`.

```sh
# Presets are user-defined; aix ships no prompt catalog.
aix prompt --list
aix prompt --json --list

# Only explicitly piped or named-file context is supplied.
cat meeting-notes.txt | aix prompt summarize
journalctl -u nginx -n 100 | aix prompt diagnose
aix prompt extract-actions --file notes.txt
git diff | aix prompt review-code --model smart
```

`--file` can be repeated. Stdin and files are passed as opaque context with the
same boundaries and ordering as `aix ask`. Preset text is sent literally: there
is no template substitution, shell execution, or environment expansion. Listing
sorts names and shows only model overrides; JSON entries contain `name`,
`model`, and `has_system`, never prompt or system bodies. Presets do not inspect
the working directory, Git state, project files, or a coding harness.

Home Manager exposes the same settings declaratively:

```nix
programs.aix.prompts = {
  summarize.prompt = "Summarize the supplied material clearly and concisely.";
  diagnose = {
    prompt = "Analyze the supplied diagnostic output.";
    system = "Distinguish evidence from inference.";
    model = "smart";
  };
};
```
