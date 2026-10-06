#!/usr/bin/env python3
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

source = Path(os.environ["PI_CODING_AGENT_DIR"]).resolve()
base_url = os.environ["OPENAI_BASE_URL"]
api_key = os.environ["OPENAI_API_KEY"]
stage_root = Path(os.environ["AIX_PI_STAGE_ROOT"])
pi = os.environ["AIX_PI_BINARY"]
args = sys.argv[1:]
option_args = args[: args.index("--")] if "--" in args else args
source_models = json.loads((source / "models.json").read_text())
allowed_models = {
    model["id"]
    for model in source_models.get("providers", {}).get("openai", {}).get("models", [])
    if isinstance(model, dict) and isinstance(model.get("id"), str)
}


def reject(message):
    print(message, file=sys.stderr)
    raise SystemExit(64)


# Pi has no --agent-dir flag; AIX owns that environment selector. A user-supplied
# API key would override the selected AIX profile and is therefore contradictory.
owned_options = ("--api-key", "--agent-dir", "--models", "--extension", "-e")
if any(
    arg == option or arg.startswith(option + "=")
    for option in owned_options
    for arg in option_args
):
    reject("aix owns Pi's agent directory, credentials, and model scope")


def option_values(option):
    values = []
    index = 0
    while index < len(option_args):
        arg = option_args[index]
        if arg == option:
            if index + 1 >= len(option_args):
                reject(f"missing value for {option}")
            values.append(option_args[index + 1])
            index += 2
            continue
        elif arg.startswith(option + "="):
            values.append(arg.split("=", 1)[1])
        index += 1
    return values


if any(provider != "openai" for provider in option_values("--provider")):
    reject("this AIX profile only authorizes OpenAI-compatible models")


def validate_model(value):
    model_id = value.split(":", 1)[0]
    if model_id.startswith("openai/"):
        model_id = model_id.removeprefix("openai/")
    if model_id not in allowed_models:
        reject(f"model {value!r} is not declared by the selected AIX profile")


for model in option_values("--model"):
    validate_model(model)

with tempfile.TemporaryDirectory(prefix="pi-agent-overlay-", dir=stage_root) as work:
    staged_profile = Path(work) / source.parent.name
    staged_agent = staged_profile / source.name
    staged_agent.mkdir(parents=True)

    # The source settings keep their original relative base: shared resources
    # outside <agent-dir> are mirrored by a symlink at the same relative path.
    shared = source.parent / "shared"
    if shared.exists():
        (staged_profile / "shared").symlink_to(shared, target_is_directory=True)

    for item in source.iterdir():
        destination = staged_agent / item.name
        if item.name == "models.json":
            models = json.loads(item.read_text())
            provider = models.get("providers", {}).get("openai")
            if not isinstance(provider, dict):
                reject("the selected AIX profile has no OpenAI-compatible provider")
            provider["baseUrl"] = base_url
            for model in provider.get("models", []):
                if isinstance(model, dict):
                    model["baseUrl"] = base_url
            destination.write_text(json.dumps(models, indent=2) + "\n")
        elif item.name == "settings.json":
            settings = json.loads(item.read_text())
            if settings.get("defaultProvider") not in (None, "openai"):
                reject("this AIX profile only authorizes OpenAI-compatible models")
            default_model = settings.get("defaultModel")
            if isinstance(default_model, str):
                validate_model(default_model)
            settings["defaultProvider"] = "openai"
            destination.write_text(json.dumps(settings, indent=2) + "\n")
        elif item.name in {"auth.json", "mcp-auth.json", "trust.json", "sessions"}:
            target = item.resolve() if item.is_symlink() else item
            destination.symlink_to(target, target_is_directory=item.is_dir())
        elif item.is_dir():
            destination.symlink_to(item.resolve(), target_is_directory=True)
        elif item.is_symlink():
            destination.symlink_to(item.resolve())
        else:
            shutil.copy2(item, destination)

    env = os.environ.copy()
    env["PI_CODING_AGENT_DIR"] = str(staged_agent)
    pi_args = [pi, *args] if args[:1] == ["auth"] else [pi, "--api-key", api_key, *args]
    result = subprocess.run(
        pi_args,
        env=env,
        check=False,
    )
    raise SystemExit(result.returncode)
