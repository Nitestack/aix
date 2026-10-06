#!/usr/bin/env python3
"""Offline Pi/aix profile-config replacement proof using synthetic credentials."""

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import http.server
import json
import os
import subprocess
import tempfile
import threading
from pathlib import Path
from typing import Any


PI_VERSION = "1.0.4"
SAVED_AUTH_KEY = "synthetic-saved-user-key"
MODEL_CONFIG_KEY = "synthetic-models-json-key"
PROFILE_A_KEY = "synthetic-aix-profile-a-key"
PROFILE_B_KEY = "synthetic-aix-profile-b-key"


class MockOpenAIServer(http.server.ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, profile: str) -> None:
        super().__init__(("127.0.0.1", 0), MockOpenAIHandler)
        self.profile = profile
        self.requests: list[dict[str, Any]] = []
        self.lock = threading.Lock()


class MockOpenAIHandler(http.server.BaseHTTPRequestHandler):
    server: MockOpenAIServer

    def log_message(self, _format: str, *_args: object) -> None:
        pass

    def do_POST(self) -> None:
        if self.path != "/v1/chat/completions":
            self.send_error(404)
            return
        try:
            body = json.loads(
                self.rfile.read(int(self.headers.get("content-length", "0")))
            )
        except (ValueError, json.JSONDecodeError):
            self.send_error(400)
            return
        request = {
            "authorization": self.headers.get("authorization"),
            "body": body,
        }
        with self.server.lock:
            self.server.requests.append(request)

        model = body.get("model", "proof-model")
        chunks = [
            {
                "id": "chatcmpl-aix-proof",
                "object": "chat.completion.chunk",
                "created": 1,
                "model": model,
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "role": "assistant",
                            "content": f"reply:{self.server.profile}",
                        },
                        "finish_reason": None,
                    }
                ],
            },
            {
                "id": "chatcmpl-aix-proof",
                "object": "chat.completion.chunk",
                "created": 1,
                "model": model,
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            },
        ]
        payload = "".join(f"data: {json.dumps(chunk)}\n\n" for chunk in chunks)
        payload += "data: [DONE]\n\n"
        encoded = payload.encode()
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("content-length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


WRAPPER_SOURCE = r"""#!/usr/bin/env python3
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
option_args = args[:args.index("--")] if "--" in args else args
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
owned_options = ("--api-key", "--agent-dir", "--models")
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
            models["providers"]["openai"]["baseUrl"] = base_url
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
    pi_args = (
        [pi, *args]
        if args[:1] == ["auth"]
        else [pi, "--api-key", api_key, *args]
    )
    result = subprocess.run(
        pi_args,
        env=env,
        check=False,
    )
    raise SystemExit(result.returncode)
"""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def model_config(model_id: str, base_url: str) -> dict[str, object]:
    return {
        "providers": {
            "openai": {
                "baseUrl": base_url,
                "api": "openai-completions",
                "apiKey": MODEL_CONFIG_KEY,
                "models": [
                    {
                        "id": model_id,
                        "name": model_id,
                        "contextWindow": 8192,
                        "maxTokens": 2048,
                    }
                ],
            }
        }
    }


def write_extension(path: Path, name: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "export default function (pi) {\n"
        "  pi.registerTool({\n"
        f"    name: {json.dumps(name)},\n"
        f"    label: {json.dumps(name)},\n"
        "    description: 'Synthetic config replacement marker',\n"
        "    parameters: { type: 'object', properties: {}, required: [] },\n"
        "    async execute() { return { content: [{ type: 'text', text: 'proof' }] }; },\n"
        "  });\n"
        "}\n"
    )


def profile_config(
    profile_dir: Path, model_id: str, theme: str, keybinding: str
) -> Path:
    agent = profile_dir / "agent"
    shared = profile_dir / "shared"
    profile_marker = profile_dir.name.replace("-", "_").upper()
    agent.mkdir(parents=True)
    shared.mkdir(parents=True)

    write_json(
        agent / "settings.json",
        {
            "theme": theme,
            "defaultProvider": "openai",
            "defaultModel": model_id,
            "extensions": ["../shared/extensions/profile.js"],
            "skills": ["../shared/skills"],
        },
    )
    write_json(
        agent / "keybindings.json",
        {"tui.editor.deleteWordBackward": keybinding},
    )
    write_json(
        agent / "models.json", model_config(model_id, "http://source.invalid/v1")
    )
    (agent / "AGENTS.md").write_text(f"{profile_marker}_AGENT_CONTEXT_MARKER\n")
    (shared / "extensions").mkdir()
    write_extension(
        shared / "extensions/profile.js",
        f"{profile_dir.name.replace('-', '_')}_only_tool",
    )
    skill_dir = shared / "skills/profile-proof"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text(
        "---\nname: profile-proof-skill\ndescription: profile skill proof "
        f"{profile_marker}_SKILL_MARKER\n---\n"
        f"{profile_marker}_SKILL_MARKER\n"
    )

    # Package configuration is treated as a read-only declarative source.
    for path in agent.iterdir():
        if not path.is_symlink() and path.is_file():
            path.chmod(0o444)
    return agent


def create_normal_agent(agent_dir: Path, model_id: str, server_url: str) -> None:
    agent_dir.mkdir(parents=True)
    (agent_dir / "sessions").mkdir()
    write_json(
        agent_dir / "settings.json",
        {
            "theme": "dark",
            "defaultProvider": "openai",
            "defaultModel": "normal-only-model",
        },
    )
    write_json(
        agent_dir / "keybindings.json",
        {"tui.editor.deleteWordBackward": "ctrl+alt+g"},
    )
    write_json(agent_dir / "models.json", model_config(model_id, server_url))
    write_json(
        agent_dir / "auth.json",
        {"openai": {"type": "api_key", "key": SAVED_AUTH_KEY}},
    )
    (agent_dir / "AGENTS.md").write_text("NORMAL_GLOBAL_CONTEXT_MARKER\n")
    (agent_dir / "extensions").mkdir()
    write_extension(agent_dir / "extensions/global-only.js", "normal_global_only_tool")
    for path in agent_dir.iterdir():
        if path.name != "sessions" and path.is_file():
            path.chmod(0o444)


def run_process(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: int = 90,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"command failed ({result.returncode}): {command!r}\n"
            f"stdout:\n{result.stdout[-3000:]}\nstderr:\n{result.stderr[-3000:]}"
        )
    return result


def list_models(pi: str, agent_dir: Path, cwd: Path, query: str) -> str:
    env = os.environ.copy()
    env.update(
        {
            "PI_CODING_AGENT_DIR": str(agent_dir),
            "PI_OFFLINE": "1",
            "PI_TELEMETRY": "0",
            "PI_SKIP_VERSION_CHECK": "1",
            "OPENAI_API_KEY": "synthetic-list-model-key",
        }
    )
    return run_process([pi, "--list-models", query], cwd=cwd, env=env).stdout


def run_staged_pi(
    wrapper: Path,
    *,
    pi: str,
    source_agent: Path,
    stage_root: Path,
    gateway_url: str,
    profile: str,
    cwd: Path,
    args: list[str],
    api_key: str,
    session_dir: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env.update(
        {
            "PI_CODING_AGENT_DIR": str(source_agent),
            "OPENAI_API_KEY": api_key,
            "OPENAI_BASE_URL": f"{gateway_url}/v1",
            "AIX_PROFILE": profile,
            "AIX_PI_STAGE_ROOT": str(stage_root),
            "AIX_PI_BINARY": pi,
            "PI_OFFLINE": "1",
            "PI_TELEMETRY": "0",
            "PI_SKIP_VERSION_CHECK": "1",
        }
    )
    if session_dir is not None:
        env["PI_CODING_AGENT_SESSION_DIR"] = str(session_dir)
    return run_process([str(wrapper), *args], cwd=cwd, env=env)


def inspect_settings_and_keys(
    pi_package: Path, agent_dir: Path, cwd: Path
) -> dict[str, Any]:
    code = r"""import { SettingsManager } from "./dist/core/settings-manager.js";
import { KeybindingsManager } from "./dist/core/keybindings.js";
const agentDir = process.env.PI_CODING_AGENT_DIR;
const settings = SettingsManager.create(process.env.PI_PROOF_CWD, agentDir, { projectTrusted: false });
const keybindings = KeybindingsManager.create(agentDir).getEffectiveConfig();
console.log(JSON.stringify({
  model: settings.getDefaultModel(),
  theme: settings.getTheme(),
  keybinding: keybindings["tui.editor.deleteWordBackward"],
}));
"""
    env = os.environ.copy()
    env.update({"PI_CODING_AGENT_DIR": str(agent_dir), "PI_PROOF_CWD": str(cwd)})
    result = run_process(
        ["node", "--input-type=module", "-e", code],
        cwd=pi_package,
        env=env,
    )
    return json.loads(result.stdout)


def request_text(messages: list[dict[str, Any]]) -> str:
    return "\n".join(
        block.get("text", "") if isinstance(block, dict) else str(block)
        for message in messages
        for block in [message.get("content", "")]
    )


def tool_names(request: dict[str, Any]) -> set[str]:
    result = set()
    for tool in request["body"].get("tools", []):
        function = tool.get("function", {})
        name = function.get("name", tool.get("name"))
        if isinstance(name, str):
            result.add(name)
    return result


def find_request(server: MockOpenAIServer, marker: str) -> dict[str, Any]:
    with server.lock:
        requests = list(server.requests)
    for request in requests:
        messages = request["body"].get("messages", [])
        if marker in request_text(messages):
            return request
    raise RuntimeError(f"mock gateway did not receive marker {marker!r}")


def make_aix_config(
    path: Path,
    *,
    aix_profile_a_url: str,
    aix_profile_b_url: str,
    profile_a_agent: Path,
    profile_b_agent: Path,
    normal_session_dir: Path,
    wrapper: Path,
    stage_root: Path,
    pi: str,
) -> None:
    quote = json.dumps
    path.write_text(
        "\n".join(
            [
                "[endpoint]",
                f"base_url = {quote(aix_profile_a_url)}",
                "",
                "[profiles.profile_a]",
                'api_key = { env = "AIX_TEST_PROFILE_A_KEY" }',
                f"base_url = {quote(aix_profile_a_url)}",
                "",
                "[profiles.profile_a.env]",
                f"PI_CODING_AGENT_DIR = {quote(str(profile_a_agent))}",
                f"PI_CODING_AGENT_SESSION_DIR = {quote(str(normal_session_dir))}",
                f"AIX_PI_STAGE_ROOT = {quote(str(stage_root))}",
                f"AIX_PI_BINARY = {quote(pi)}",
                "",
                "[profiles.profile_b]",
                'api_key = { env = "AIX_TEST_PROFILE_B_KEY" }',
                f"base_url = {quote(aix_profile_b_url)}",
                "",
                "[profiles.profile_b.env]",
                f"PI_CODING_AGENT_DIR = {quote(str(profile_b_agent))}",
                f"PI_CODING_AGENT_SESSION_DIR = {quote(str(normal_session_dir))}",
                f"AIX_PI_STAGE_ROOT = {quote(str(stage_root))}",
                f"AIX_PI_BINARY = {quote(pi)}",
                "",
                "[tools.pi]",
                f"command = {quote(str(wrapper))}",
                'api_format = "openai"',
                "",
            ]
        )
    )


def native_project_session_dir(agent_dir: Path, cwd: Path) -> Path:
    resolved = str(cwd.resolve())
    safe_path = resolved[1:] if resolved.startswith(("/", "\\")) else resolved
    for separator in ("/", "\\", ":"):
        safe_path = safe_path.replace(separator, "-")
    return agent_dir / "sessions" / f"--{safe_path}--"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pi", required=True, help="path to Pi 1.0.4 executable")
    parser.add_argument("--aix", required=True, help="path to the aix executable")
    parser.add_argument("--tmp-root", default="/tmp/opencode")
    args = parser.parse_args()

    pi = str(Path(args.pi).resolve())
    aix = str(Path(args.aix).resolve())
    pi_package = Path(pi).resolve().parents[2]
    version = run_process(
        [pi, "--version"], cwd=Path.cwd(), env=os.environ.copy()
    ).stdout.strip()
    require(version == PI_VERSION, f"expected Pi {PI_VERSION}, found {version}")

    tmp_root = Path(args.tmp_root)
    tmp_root.mkdir(parents=True, exist_ok=True)
    temporary = tempfile.TemporaryDirectory(prefix="aix-pi-config-proof-", dir=tmp_root)
    servers = [MockOpenAIServer("profile-a"), MockOpenAIServer("profile-b")]
    threads = [
        threading.Thread(target=server.serve_forever, daemon=True) for server in servers
    ]
    for thread in threads:
        thread.start()

    try:
        root = Path(temporary.name)
        profile_a_server, profile_b_server = servers
        profile_a_url = f"http://127.0.0.1:{profile_a_server.server_port}"
        profile_b_url = f"http://127.0.0.1:{profile_b_server.server_port}"
        normal_agent = root / "normal" / "agent"
        profile_a_root = root / "profiles" / "profile-a"
        profile_b_root = root / "profiles" / "profile-b"
        profile_a_agent = profile_config(
            profile_a_root, "profile-a-model", "light", "ctrl+alt+a"
        )
        profile_b_agent = profile_config(
            profile_b_root, "profile-b-model", "dark", "ctrl+alt+b"
        )
        create_normal_agent(normal_agent, "normal-session-model", f"{profile_a_url}/v1")
        for agent in (profile_a_agent, profile_b_agent):
            (agent / "auth.json").symlink_to(normal_agent / "auth.json")
        stage_root = root / "staging"
        stage_root.mkdir()

        project = root / "project"
        project.mkdir()
        normal_session_dir = native_project_session_dir(normal_agent, project)
        (project / "AGENTS.md").write_text("PROJECT_CONTEXT_MARKER\n")
        project_pi = project / ".pi"
        project_pi.mkdir()
        (project_pi / "APPEND_SYSTEM.md").write_text("PROJECT_APPROVED_CONFIG_MARKER\n")
        (project_pi / "settings.json").write_text(
            json.dumps({"defaultThinkingLevel": "low"})
        )
        write_extension(
            project_pi / "extensions" / "project-only.js", "project_only_tool"
        )

        wrapper = root / "pi-aix-wrapper.py"
        wrapper.write_text(WRAPPER_SOURCE)
        wrapper.chmod(0o700)
        aix_config = root / "aix.toml"
        make_aix_config(
            aix_config,
            aix_profile_a_url=profile_a_url,
            aix_profile_b_url=profile_b_url,
            profile_a_agent=profile_a_agent,
            profile_b_agent=profile_b_agent,
            normal_session_dir=normal_session_dir,
            wrapper=wrapper,
            stage_root=stage_root,
            pi=pi,
        )

        env = os.environ.copy()
        env.update(
            {
                "PI_CODING_AGENT_DIR": str(root / "wrong-inherited-agent"),
                "PI_CODING_AGENT_SESSION_DIR": str(root / "wrong-inherited-sessions"),
                "OPENAI_API_KEY": "synthetic-inherited-key-must-not-win",
                "OPENAI_BASE_URL": "http://127.0.0.1:1/should-not-win",
                "AIX_TEST_PROFILE_A_KEY": PROFILE_A_KEY,
                "AIX_TEST_PROFILE_B_KEY": PROFILE_B_KEY,
                "AIX_STATE_DIR": str(root / "aix-state"),
                "PI_OFFLINE": "1",
                "PI_TELEMETRY": "0",
                "PI_SKIP_VERSION_CHECK": "1",
            }
        )
        for proxy_name in (
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ):
            env.pop(proxy_name, None)

        selected_env = json.loads(
            run_process(
                [
                    aix,
                    "--config",
                    str(aix_config),
                    "--profile",
                    "profile_a",
                    "--non-interactive",
                    "env",
                    "--format",
                    "json",
                ],
                cwd=project,
                env=env,
            ).stdout
        )
        require(
            selected_env.get("PI_CODING_AGENT_DIR") == str(profile_a_agent)
            and selected_env.get("PI_CODING_AGENT_SESSION_DIR")
            == str(normal_session_dir),
            f"aix did not select the profile config and shared session dirs: {selected_env}",
        )

        # Prove the default global directory and each selected profile are distinct.
        global_models = list_models(pi, normal_agent, project, "normal-only-model")
        selected_models = run_staged_pi(
            wrapper,
            pi=pi,
            source_agent=profile_a_agent,
            stage_root=stage_root,
            gateway_url=profile_a_url,
            profile="profile_a",
            cwd=project,
            args=["--list-models", "profile-a-model"],
            api_key=PROFILE_A_KEY,
        ).stdout
        leaked_models = run_staged_pi(
            wrapper,
            pi=pi,
            source_agent=profile_a_agent,
            stage_root=stage_root,
            gateway_url=profile_a_url,
            profile="profile_a",
            cwd=project,
            args=["--list-models", "normal-only-model"],
            api_key=PROFILE_A_KEY,
        ).stdout
        require(
            "normal-only-model" in global_models, "normal fixture model was not found"
        )
        require(
            "profile-a-model" in selected_models, "selected profile model was not found"
        )
        require(
            "No models matching" in leaked_models,
            "normal global model leaked into profile A",
        )

        normal_effective = inspect_settings_and_keys(pi_package, normal_agent, project)
        profile_a_effective = inspect_settings_and_keys(
            pi_package, profile_a_agent, project
        )
        require(normal_effective["theme"] == "dark", "normal theme was not loaded")
        require(
            normal_effective["keybinding"] == "ctrl+alt+g",
            "normal keybindings were not loaded",
        )
        require(
            profile_a_effective.get("model") == "profile-a-model",
            f"profile settings leaked: {profile_a_effective}",
        )
        require(profile_a_effective["theme"] == "light", "profile theme was not loaded")
        require(
            profile_a_effective["keybinding"] == "ctrl+alt+a",
            "profile keybindings were not loaded",
        )

        auth_result = run_staged_pi(
            wrapper,
            pi=pi,
            source_agent=profile_a_agent,
            stage_root=stage_root,
            gateway_url=profile_a_url,
            profile="profile_a",
            cwd=project,
            args=["auth", "print-api-key", "--provider", "openai"],
            api_key=PROFILE_A_KEY,
        )
        require(
            auth_result.stdout.strip() == SAVED_AUTH_KEY,
            "saved auth was not shared into profile A",
        )
        require(
            (profile_a_agent / "auth.json").resolve()
            == (normal_agent / "auth.json").resolve(),
            "profile A auth does not point at the existing shared auth file",
        )

        model_sources = {
            path: sha256(path)
            for path in (
                profile_a_agent / "settings.json",
                profile_a_agent / "models.json",
                profile_a_agent / "keybindings.json",
                profile_b_agent / "settings.json",
                profile_b_agent / "models.json",
                profile_b_agent / "keybindings.json",
                normal_agent / "auth.json",
            )
        }

        def run_aix(
            profile: str, *pi_args: str, mode: str = "run"
        ) -> subprocess.CompletedProcess[str]:
            if mode == "run":
                launch_args = ["run", "--", "pi", *pi_args]
            elif mode == "tool":
                launch_args = ["pi", "--", *pi_args]
            elif mode == "exec":
                launch_args = ["exec", "--", str(wrapper), *pi_args]
            else:
                raise ValueError(f"unknown aix launch mode: {mode}")
            return run_process(
                [
                    aix,
                    "--config",
                    str(aix_config),
                    "--profile",
                    profile,
                    "--non-interactive",
                    *launch_args,
                ],
                cwd=project,
                env=env,
            )

        # Create history in Pi's normal agent directory, then resume it using the
        # alternate directory selector and the unchanged native session directory.
        bootstrap_env = {
            **env,
            "PI_CODING_AGENT_DIR": str(normal_agent),
            "OPENAI_API_KEY": SAVED_AUTH_KEY,
        }
        bootstrap_env.pop("PI_CODING_AGENT_SESSION_DIR", None)
        session_bootstrap = run_process(
            [
                pi,
                "--print",
                "--model",
                "openai/normal-session-model",
                "--approve",
                "normal-session-existing-marker",
            ],
            cwd=project,
            env=bootstrap_env,
        )
        require(
            "reply:profile-a" in session_bootstrap.stdout,
            "normal Pi session bootstrap failed",
        )
        sessions_before_resume = list((normal_agent / "sessions").rglob("*.jsonl"))
        require(
            len(sessions_before_resume) == 1,
            "normal Pi session was not saved in the original store",
        )

        direct_resume = run_staged_pi(
            wrapper,
            pi=pi,
            source_agent=profile_a_agent,
            stage_root=stage_root,
            gateway_url=profile_a_url,
            profile="profile_a",
            cwd=project,
            args=[
                "--continue",
                "--model",
                "openai/profile-a-model",
                "--print",
                "--approve",
                "profile-a-direct-resume-marker",
            ],
            api_key=PROFILE_A_KEY,
            session_dir=normal_session_dir,
        )
        direct_resume_request = find_request(
            profile_a_server, "profile-a-direct-resume-marker"
        )
        require(
            "normal-session-existing-marker"
            in request_text(direct_resume_request["body"]["messages"]),
            "Pi did not resume the normal session when given its directory directly: "
            + repr(
                [
                    {
                        "path": str(path),
                        "header": path.read_text().splitlines()[0],
                        "contains_marker": "normal-session-existing-marker"
                        in path.read_text(),
                    }
                    for path in sessions_before_resume
                ]
            ),
        )

        resumed = run_aix(
            "profile_a",
            "--continue",
            "--model",
            "openai/profile-a-model",
            "--print",
            "--approve",
            "profile-a-resume-marker",
        )
        require(
            "reply:profile-a" in resumed.stdout,
            "profile A failed to resume the existing session",
        )
        resumed_request = find_request(profile_a_server, "profile-a-resume-marker")
        resumed_text = request_text(resumed_request["body"]["messages"])
        require(
            "normal-session-existing-marker" in resumed_text,
            f"old session history was not available: {resumed_text[-3000:]}",
        )
        require(
            resumed_request["authorization"] == f"Bearer {PROFILE_A_KEY}",
            "AIX key did not override saved auth",
        )

        named = run_aix(
            "profile_a",
            "--model",
            "openai/profile-a-model",
            "--print",
            "--approve",
            "profile-a-named-tool-marker",
            mode="tool",
        )
        require("reply:profile-a" in named.stdout, "aix pi launch failed")
        named_request = find_request(profile_a_server, "profile-a-named-tool-marker")
        require(
            named_request["authorization"] == f"Bearer {PROFILE_A_KEY}",
            "aix pi did not keep the profile connection authoritative",
        )

        executed = run_aix(
            "profile_a",
            "--provider",
            "openai",
            "--model",
            "openai/profile-a-model",
            "--print",
            "--approve",
            "profile-a-exec-marker",
            mode="exec",
        )
        require("reply:profile-a" in executed.stdout, "aix exec launch failed")
        exec_request = find_request(profile_a_server, "profile-a-exec-marker")
        require(
            exec_request["authorization"] == f"Bearer {PROFILE_A_KEY}",
            "aix exec did not keep the profile connection authoritative",
        )

        # Different profiles launch at the same time, sharing the existing session
        # store but selecting different config roots, keys, and gateway endpoints.
        prompts = {
            "profile_a": "profile-a-concurrent-marker",
            "profile_b": "profile-b-concurrent-marker",
        }
        sessions_before_concurrent = list((normal_agent / "sessions").rglob("*.jsonl"))
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
            futures = {
                profile: executor.submit(
                    run_aix,
                    profile,
                    "--model",
                    f"openai/{profile.replace('_', '-')}-model",
                    "--print",
                    "--approve" if profile == "profile_a" else "--no-approve",
                    prompts[profile],
                )
                for profile in prompts
            }
            results = {profile: future.result() for profile, future in futures.items()}

        require(
            "reply:profile-a" in results["profile_a"].stdout,
            "AIX profile A launch failed",
        )
        require(
            "reply:profile-b" in results["profile_b"].stdout,
            "AIX profile B launch failed",
        )
        a_request = find_request(profile_a_server, prompts["profile_a"])
        b_request = find_request(profile_b_server, prompts["profile_b"])
        require(
            a_request["authorization"] == f"Bearer {PROFILE_A_KEY}",
            "profile A key crossed over",
        )
        require(
            b_request["authorization"] == f"Bearer {PROFILE_B_KEY}",
            "profile B key crossed over",
        )
        a_text = request_text(a_request["body"]["messages"])
        b_text = request_text(b_request["body"]["messages"])
        require(
            "PROFILE_A_AGENT_CONTEXT_MARKER" in a_text,
            "profile A instructions were not loaded",
        )
        require(
            "PROFILE_A_SKILL_MARKER" in a_text,
            "relative skill outside agent dir was not loaded",
        )
        require(
            "NORMAL_GLOBAL_CONTEXT_MARKER" not in a_text,
            "normal global instructions leaked",
        )
        require(
            "PROJECT_APPROVED_CONFIG_MARKER" in a_text,
            "approved project config was not loaded",
        )
        require(
            "PROJECT_CONTEXT_MARKER" in a_text, "project instructions were not loaded"
        )
        require(
            "PROFILE_B_AGENT_CONTEXT_MARKER" in b_text,
            "profile B instructions were not loaded",
        )
        require(
            "PROJECT_APPROVED_CONFIG_MARKER" not in b_text,
            "--no-approve did not honor Pi trust rules",
        )
        require(
            "PROJECT_CONTEXT_MARKER" in b_text, "project context discovery was disabled"
        )
        a_tools = tool_names(a_request)
        b_tools = tool_names(b_request)
        require(
            "profile_a_only_tool" in a_tools,
            "profile A relative extension was not loaded",
        )
        require(
            "profile_b_only_tool" in b_tools,
            "profile B relative extension was not loaded",
        )
        require(
            "normal_global_only_tool" not in a_tools | b_tools,
            "normal global extension leaked",
        )
        require(
            "project_only_tool" in a_tools, "approved project extension was not loaded"
        )
        require(
            "project_only_tool" not in b_tools,
            "unapproved project extension was loaded",
        )

        sessions_after = list((normal_agent / "sessions").rglob("*.jsonl"))
        require(
            len(sessions_after) == len(sessions_before_concurrent) + 2,
            "concurrent runs did not share the original session store cleanly",
        )
        require(
            all(sha256(path) == digest for path, digest in model_sources.items()),
            "a Pi/AIX run modified a read-only configuration/auth source",
        )

        # The wrapper's selector guard is tested without a gateway call. Run it
        # directly with the AIX-selected environment, as an explicit API-key
        # argument must never override the profile credential.
        reject_env = env.copy()
        reject_env.update(
            {
                "PI_CODING_AGENT_DIR": str(profile_a_agent),
                "OPENAI_API_KEY": PROFILE_A_KEY,
                "OPENAI_BASE_URL": f"{profile_a_url}/v1",
                "AIX_PROFILE": "profile_a",
                "AIX_PI_STAGE_ROOT": str(stage_root),
                "AIX_PI_BINARY": pi,
            }
        )
        for conflict_args, conflict_name in (
            (["--api-key", "synthetic-contradictory-key"], "API key"),
            (["--api-key=synthetic-contradictory-key"], "API key"),
            (["--agent-dir", str(root / "other-agent")], "agent directory"),
            (["--models", "openai/*"], "model-cycle selector"),
            (["--provider", "anthropic"], "provider"),
            (["--model", "anthropic/claude"], "model provider"),
            (["--model", "openai/unconfigured-model"], "model outside profile config"),
        ):
            rejected = subprocess.run(
                [str(wrapper), *conflict_args],
                cwd=project,
                env=reject_env,
                capture_output=True,
                text=True,
                check=False,
            )
            require(
                rejected.returncode == 64,
                f"contradictory native {conflict_name} argument was not rejected",
            )

        print(f"pi_version={version}")
        print("config_replacement=settings+models+keybindings+resources=passed")
        print("project_rules_and_trust=passed")
        print("session_history_and_saved_auth=passed")
        print("aix_run_named_tool_exec_and_concurrent_profiles=passed")
        print("read_only_sources_and_relative_resources=passed")
    finally:
        for server in servers:
            server.shutdown()
            server.server_close()
        for thread in threads:
            thread.join(timeout=2)
        temporary.cleanup()


if __name__ == "__main__":
    try:
        main()
    except (
        OSError,
        RuntimeError,
        json.JSONDecodeError,
        subprocess.TimeoutExpired,
    ) as error:
        raise SystemExit(
            f"Pi agent config replacement proof failed: {error}"
        ) from error
