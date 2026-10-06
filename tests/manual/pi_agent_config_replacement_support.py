"""Shared fixtures and helpers for the manual Pi configuration proof."""

from __future__ import annotations

import hashlib
import http.server
import json
import os
import subprocess
import threading
from dataclasses import dataclass
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


@dataclass
class ProofContext:
    aix: str
    pi: str
    pi_package: Path
    root: Path
    project: Path
    normal_agent: Path
    normal_session_dir: Path
    profile_a_agent: Path
    profile_b_agent: Path
    profile_a_server: MockOpenAIServer
    profile_b_server: MockOpenAIServer
    profile_a_url: str
    profile_b_url: str
    stage_root: Path
    wrapper: Path
    aix_config: Path
    env: dict[str, str]


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
                        "baseUrl": base_url,
                        "apiKey": MODEL_CONFIG_KEY,
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


def run_aix(
    proof: ProofContext, profile: str, *pi_args: str, mode: str = "run"
) -> subprocess.CompletedProcess[str]:
    if mode == "run":
        launch_args = ["run", "--", "pi", *pi_args]
    elif mode == "tool":
        launch_args = ["pi", "--", *pi_args]
    elif mode == "exec":
        launch_args = ["exec", "--", str(proof.wrapper), *pi_args]
    else:
        raise ValueError(f"unknown aix launch mode: {mode}")
    return run_process(
        [
            proof.aix,
            "--config",
            str(proof.aix_config),
            "--profile",
            profile,
            "--non-interactive",
            *launch_args,
        ],
        cwd=proof.project,
        env=proof.env,
    )


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
    proof: ProofContext,
    *,
    source_agent: Path,
    profile: str,
    args: list[str],
    api_key: str,
    session_dir: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    gateway_url = {
        "profile_a": proof.profile_a_url,
        "profile_b": proof.profile_b_url,
    }[profile]
    env = proof.env.copy()
    env.update(
        {
            "PI_CODING_AGENT_DIR": str(source_agent),
            "OPENAI_API_KEY": api_key,
            "OPENAI_BASE_URL": f"{gateway_url}/v1",
            "AIX_PROFILE": profile,
            "AIX_PI_STAGE_ROOT": str(proof.stage_root),
            "AIX_PI_BINARY": proof.pi,
            "PI_OFFLINE": "1",
            "PI_TELEMETRY": "0",
            "PI_SKIP_VERSION_CHECK": "1",
        }
    )
    if session_dir is not None:
        env["PI_CODING_AGENT_SESSION_DIR"] = str(session_dir)
    return run_process([str(proof.wrapper), *args], cwd=proof.project, env=env)


def inspect_settings_and_keys(
    pi_package: Path,
    agent_dir: Path,
    cwd: Path,
    *,
    project_trusted: bool = False,
) -> dict[str, Any]:
    code = r"""import { SettingsManager } from "./dist/core/settings-manager.js";
import { KeybindingsManager } from "./dist/core/keybindings.js";
const agentDir = process.env.PI_CODING_AGENT_DIR;
const settings = SettingsManager.create(process.env.PI_PROOF_CWD, agentDir, {
  projectTrusted: process.env.PI_PROOF_PROJECT_TRUSTED === "true",
});
const keybindings = KeybindingsManager.create(agentDir).getEffectiveConfig();
console.log(JSON.stringify({
  model: settings.getDefaultModel(),
  theme: settings.getTheme(),
  thinking: settings.getDefaultThinkingLevel(),
  keybinding: keybindings["tui.editor.deleteWordBackward"],
}));
"""
    env = os.environ.copy()
    env.update(
        {
            "PI_CODING_AGENT_DIR": str(agent_dir),
            "PI_PROOF_CWD": str(cwd),
            "PI_PROOF_PROJECT_TRUSTED": str(project_trusted).lower(),
        }
    )
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
