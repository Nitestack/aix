#!/usr/bin/env python3
"""Manual, offline-feasible OpenCode V2 config-replacement proof for issue #33.

Requires the exact OpenCode v2.0.20 CLI, Python 3, and an XDG-compatible host.
Only disposable files under /tmp/opencode and a loopback mock model endpoint are
used. No existing OpenCode data, auth, or shared server is opened.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any


VERSION = "opencode v2.0.20"
AUTH_FIXTURE = "ISSUE33_DUMMY_AUTH_KEY_NEVER_PRINT"
API_FIXTURE = "ISSUE33_DUMMY_MODEL_KEY"
BRIDGE_FIXTURE = "ISSUE33_DUMMY_BRIDGE_KEY"
PROJECT_MARKER = "ISSUE33_PROJECT_INSTRUCTION_MARKER"
GLOBAL_MARKER = "ISSUE33_GLOBAL_CONFIG_MUST_NOT_LEAK"
GLOBAL_CLI_PLUGIN = "issue33-global-cli-only-plugin"
PROFILE_CLI_PLUGIN = "issue33-profile-cli-only-plugin"
BRIDGE_MODEL = "gpt-6-luna"


class MockModelHandler(BaseHTTPRequestHandler):
    def log_message(self, _format: str, *_args: Any) -> None:
        pass

    def do_GET(self) -> None:
        if self.path.rstrip("/").endswith("/models"):
            self._json({"object": "list", "data": []})
            return
        self.send_error(404)

    def do_POST(self) -> None:
        length = int(self.headers.get("content-length", "0"))
        body = json.loads(self.rfile.read(length))
        self.server.requests.append(  # type: ignore[attr-defined]
            {
                "path": self.path,
                "authorization": self.headers.get("authorization"),
                "body": body,
            }
        )
        if body.get("stream"):
            if self.path.rstrip("/").endswith("/responses"):
                events = [
                    {
                        "type": "response.created",
                        "response": {"id": "resp-issue33"},
                    },
                    {
                        "type": "response.output_item.added",
                        "item": {"type": "message", "id": "msg-issue33"},
                    },
                    {
                        "type": "response.output_text.delta",
                        "item_id": "msg-issue33",
                        "delta": "fixture response",
                    },
                    {
                        "type": "response.completed",
                        "response": {
                            "id": "resp-issue33",
                            "usage": {
                                "input_tokens": 1,
                                "output_tokens": 2,
                                "total_tokens": 3,
                            },
                        },
                    },
                ]
                data = b"".join(
                    b"data: " + json.dumps(event).encode() + b"\n\n" for event in events
                )
                data += b"data: [DONE]\n\n"
                self.send_response(200)
                self.send_header("content-type", "text/event-stream")
                self.send_header("content-length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
                return
            chunks = [
                {
                    "id": "chatcmpl-issue33",
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": body.get("model", "fixture"),
                    "choices": [
                        {
                            "index": 0,
                            "delta": {
                                "role": "assistant",
                                "content": "fixture response",
                            },
                            "finish_reason": None,
                        }
                    ],
                },
                {
                    "id": "chatcmpl-issue33",
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": body.get("model", "fixture"),
                    "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                },
            ]
            data = b"".join(
                b"data: " + json.dumps(chunk).encode() + b"\n\n" for chunk in chunks
            )
            data += b"data: [DONE]\n\n"
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        self._json(
            {
                "id": "chatcmpl-issue33",
                "object": "chat.completion",
                "created": 0,
                "model": body.get("model", "fixture"),
                "choices": [
                    {
                        "index": 0,
                        "message": {"role": "assistant", "content": "fixture response"},
                        "finish_reason": "stop",
                    }
                ],
                "usage": {
                    "prompt_tokens": 1,
                    "completion_tokens": 1,
                    "total_tokens": 2,
                },
            }
        )

    def _json(self, value: Any) -> None:
        data = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def invoke(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    expect: int = 0,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        text=True,
        capture_output=True,
        timeout=90,
        check=False,
    )
    output = result.stdout + result.stderr
    require(AUTH_FIXTURE not in output, "OpenCode printed the dummy saved-auth key")
    require(API_FIXTURE not in output, "OpenCode printed the dummy API key")
    require(BRIDGE_FIXTURE not in output, "OpenCode printed the dummy bridge bearer")
    if result.returncode != expect:
        details = output.replace(API_FIXTURE, "[redacted fixture key]").replace(
            BRIDGE_FIXTURE, "[redacted bridge fixture]"
        )
        raise RuntimeError(
            f"command exited {result.returncode}: {command!r}\n{details}"
        )
    return result


def json_file(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def config_entries(
    binary: str, project: Path, env: dict[str, str]
) -> list[dict[str, Any]]:
    result = invoke(
        [binary, "api", "--standalone", "config.get"],
        cwd=project,
        env=env,
    )
    return json.loads(result.stdout)


def documents(entries: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [entry for entry in entries if entry.get("type") == "document"]


def path_map(binary: str, cwd: Path, env: dict[str, str]) -> dict[str, str]:
    output = invoke([binary, "debug", "paths"], cwd=cwd, env=env).stdout
    return {
        key.strip(): value.strip()
        for key, value in (
            line.split(None, 1) for line in output.splitlines() if line.strip()
        )
    }


def all_strings(value: Any) -> list[str]:
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        return [item for child in value for item in all_strings(child)]
    if isinstance(value, dict):
        return [item for child in value.values() for item in all_strings(child)]
    return []


def fingerprint(paths: list[Path]) -> dict[Path, str]:
    return {path: hashlib.sha256(path.read_bytes()).hexdigest() for path in paths}


def main() -> None:
    binary = os.environ.get("OPENCODE_BIN", "opencode")
    if shutil.which(binary) is None and not Path(binary).is_file():
        raise RuntimeError("Set OPENCODE_BIN or put OpenCode v2.0.20 on PATH")
    version = subprocess.run(
        [binary, "--version"], check=True, text=True, capture_output=True
    ).stdout.strip()
    require(version == VERSION, f"tested boundary is {VERSION!r}, found {version!r}")

    scratch_parent = Path("/tmp/opencode")
    if not scratch_parent.is_dir():
        raise RuntimeError(
            "Create /tmp/opencode first; the proof intentionally keeps scratch data there"
        )

    with tempfile.TemporaryDirectory(
        prefix="aix-issue-33-", dir=scratch_parent
    ) as temporary:
        root = Path(temporary)
        home = root / "home"
        xdg_config = root / "xdg-config"
        standard = xdg_config / "opencode"
        data_home = root / "xdg-data"
        project = root / "project"
        profile_a_source = root / "profile-a-source"
        profile_b_source = root / "profile-b-source"
        profile_c_source = root / "profile-c-source"
        profile_a = root / "stage-a"
        profile_b = root / "stage-b"
        profile_b_cli_probe = root / "stage-b-cli-probe"
        profile_c = root / "stage-c"
        for directory in (
            home,
            standard,
            project,
            profile_a_source,
            profile_b_source,
            profile_c_source,
        ):
            directory.mkdir(parents=True, exist_ok=True)

        model_server = ThreadingHTTPServer(("127.0.0.1", 0), MockModelHandler)
        model_server.requests = []  # type: ignore[attr-defined]
        server_thread = threading.Thread(target=model_server.serve_forever, daemon=True)
        server_thread.start()
        base_url = f"http://127.0.0.1:{model_server.server_port}/v1"

        try:
            standard_app = standard / "opencode.json"
            standard_cli = standard / "cli.json"
            json_file(
                standard_app,
                {
                    "$schema": "https://opencode.ai/config.json",
                    "username": GLOBAL_MARKER,
                    "default_agent": "issue33-global-only-agent",
                },
            )
            standard_cli_value = {
                "$schema": "https://opencode.ai/v2/cli.json",
                "animations": False,
                "mouse": True,
                "plugins": [GLOBAL_CLI_PLUGIN],
            }
            json_file(standard_cli, standard_cli_value)

            project_config = project / "opencode.json"
            json_file(
                project_config,
                {"$schema": "https://opencode.ai/config.json", "share": "manual"},
            )
            (project / "AGENTS.md").write_text(
                f"When asked for the local proof marker, include {PROJECT_MARKER}.\n",
                encoding="utf-8",
            )

            app_a = profile_a_source / "opencode.json"
            app_a_resource = profile_a_source / "resources" / "username.txt"
            app_a_resource.parent.mkdir(parents=True)
            app_a_resource.write_text("profile-a-relative-resource\n", encoding="utf-8")
            json_file(
                app_a,
                {
                    "$schema": "https://opencode.ai/config.json",
                    "model": "fixture/configured",
                    "username": "{file:resources/username.txt}",
                    "providers": {
                        "fixture": {
                            "name": "Local issue #33 fixture",
                            "env": ["AIX_ISSUE33_API_KEY"],
                            "package": "@opencode/ai/providers/openai-compatible",
                            "settings": {"baseURL": base_url},
                            "models": {
                                "configured": {
                                    "name": "Configured model",
                                    "limit": {"context": 8192, "output": 1024},
                                },
                                "explicit": {
                                    "name": "Explicit model",
                                    "limit": {"context": 8192, "output": 1024},
                                },
                            },
                        }
                    },
                },
            )
            # No custom CLI config for profile A: pair its app config with the
            # unchanged standard cli.json, preserving the native file format.
            shutil.copytree(profile_a_source, profile_a)
            shutil.copy2(standard_cli, profile_a / "cli.json")

            # Profile B changes terminal preferences only. Its staged app file
            # is the untouched standard app source, while cli.json is selected
            # independently from the profile.
            cli_b = profile_b_source / "cli.json"
            json_file(
                cli_b,
                {
                    "$schema": "https://opencode.ai/v2/cli.json",
                    "animations": True,
                    "plugins": [PROFILE_CLI_PLUGIN],
                },
            )
            shutil.copytree(standard, profile_b)
            shutil.copy2(cli_b, profile_b / "cli.json")
            shutil.copytree(profile_b, profile_b_cli_probe)

            # Profile C is used only for the concurrent launch isolation check.
            app_c = profile_c_source / "opencode.json"
            json_file(
                app_c,
                {
                    "$schema": "https://opencode.ai/config.json",
                    "username": "profile-c-only",
                },
            )
            shutil.copytree(profile_c_source, profile_c)
            shutil.copy2(standard_cli, profile_c / "cli.json")

            base_env = os.environ.copy()
            for name in (
                "OPENCODE_CONFIG_DIR",
                "OPENCODE_CONFIG",
                "OPENCODE_CONFIG_CONTENT",
                "OPENCODE_CLI_CONFIG_CONTENT",
                "OPENCODE_DB",
                "OPENCODE_PASSWORD",
                "OPENCODE_SERVER_PASSWORD",
                "OPENCODE_CONFIG_PROJECT_DISABLE",
                "OPENCODE_DISABLE_PROJECT_CONFIG",
                "OPENCODE_SERVER",
                "OPENCODE_SERVER_URL",
                "AIX_OPENCODE_BRIDGE_TOKEN",
                "AIX_ISSUE33_API_KEY",
            ):
                base_env.pop(name, None)
            base_env.update(
                {
                    "HOME": str(home),
                    "OPENCODE_TEST_HOME": str(home),
                    "XDG_CONFIG_HOME": str(xdg_config),
                    "XDG_DATA_HOME": str(data_home),
                    "XDG_CACHE_HOME": str(root / "xdg-cache"),
                    "XDG_STATE_HOME": str(root / "xdg-state"),
                    "OPENCODE_DISABLE_MODELS_FETCH": "true",
                    "OPENCODE_DISABLE_FILEWATCHER": "true",
                    "AIX_ISSUE33_API_KEY": API_FIXTURE,
                    "AIX_OPENCODE_BRIDGE_TOKEN": BRIDGE_FIXTURE,
                }
            )

            source_paths = [
                standard_app,
                standard_cli,
                project_config,
                project / "AGENTS.md",
                app_a,
                app_a_resource,
                cli_b,
                app_c,
                profile_a / "opencode.json",
                profile_a / "cli.json",
                profile_b / "opencode.json",
                profile_b / "cli.json",
                profile_c / "opencode.json",
                profile_c / "cli.json",
            ]
            source_fingerprints = fingerprint(source_paths)

            # Without profile selectors the normal global source remains the
            # source OpenCode reads. --standalone keeps even this control probe
            # away from any host shared service.
            standard_entries = config_entries(binary, project, base_env)
            require(
                any(
                    doc.get("info", {}).get("username") == GLOBAL_MARKER
                    for doc in documents(standard_entries)
                ),
                "control run did not read the ordinary global app config",
            )

            # Seed only disposable, synthetic user data. The key is never put
            # into app/CLI config or emitted by the CLI.
            auth_fixture = root / "auth-fixture.json"
            json_file(
                auth_fixture,
                [
                    {
                        "id": "cred_issue33_fixture",
                        "integrationID": "openai",
                        "label": "issue33-dummy-auth",
                        "active": True,
                        "value": {"type": "key", "key": AUTH_FIXTURE},
                    }
                ],
            )
            auth_fixture.chmod(0o600)
            invoke(
                [binary, "auth", "import", "--standalone", str(auth_fixture)],
                cwd=project,
                env=base_env,
            )

            # Emulate an inherited config-dir selector, then replace it with
            # the profile's staged root. Profile A replaces app config while
            # retaining the independently selected standard terminal config.
            inherited_env = dict(base_env)
            inherited = root / "inherited-config"
            inherited.mkdir()
            json_file(
                inherited / "opencode.json",
                {"username": "inherited-selector-must-lose"},
            )
            inherited_env["OPENCODE_CONFIG_DIR"] = str(inherited)
            env_a = dict(inherited_env)
            env_a["OPENCODE_CONFIG_DIR"] = str(profile_a)

            paths_a = path_map(binary, project, env_a)
            require(
                Path(paths_a["config"]) == profile_a,
                "profile A did not select its staged config root",
            )
            require(
                Path(paths_a["data"]) == data_home / "opencode",
                "changing OPENCODE_CONFIG_DIR changed the user-data root",
            )
            require(
                json.loads((profile_a / "cli.json").read_text(encoding="utf-8"))
                == standard_cli_value,
                "profile A did not retain the standard native CLI file",
            )

            selected_entries = config_entries(binary, project, env_a)
            selected_docs = documents(selected_entries)
            selected_json = json.dumps(selected_docs)
            require(
                GLOBAL_MARKER not in selected_json,
                "replaced global app value leaked into profile A",
            )
            require(
                "issue33-global-only-agent" not in selected_json,
                "global-only default agent leaked into profile A",
            )
            require(
                "inherited-selector-must-lose" not in selected_json,
                "inherited config-dir beat the profile",
            )
            require(
                any(
                    doc.get("info", {}).get("username") == "profile-a-relative-resource"
                    for doc in selected_docs
                ),
                "relative {file:...} reference did not resolve from its selected native config file",
            )
            require(
                any(
                    doc.get("info", {}).get("share") == "manual"
                    for doc in selected_docs
                ),
                "project configuration was not loaded over the selected global replacement",
            )

            # ChatGPT's existing process-local config is an additional runtime
            # source; it must add its provider without replacing the selected
            # app config or leaking the replaced global app file back in.
            bridge_content = {
                "providers": {
                    "aix-chatgpt": {
                        "name": "Issue #33 local bridge fixture",
                        "env": ["AIX_OPENCODE_BRIDGE_TOKEN"],
                        "package": "@opencode/ai/providers/openai/responses",
                        "canonical": "openai",
                        "settings": {"baseURL": base_url, "transport": "http"},
                        "models": {BRIDGE_MODEL: {}},
                    }
                }
            }
            env_a["OPENCODE_CONFIG_CONTENT"] = json.dumps(bridge_content)
            bridge_entries = config_entries(binary, project, env_a)
            bridge_docs = documents(bridge_entries)
            bridge_json = json.dumps(bridge_docs)
            provider_ids = {
                provider_id
                for document in bridge_docs
                for provider_id in document.get("info", {}).get("providers", {})
            }
            require(
                "aix-chatgpt" in provider_ids, "runtime bridge config was not loaded"
            )
            require(
                "fixture" in provider_ids,
                "selected profile provider was overwritten by bridge config",
            )
            require(
                GLOBAL_MARKER not in bridge_json,
                "runtime bridge overlay reintroduced replaced global config",
            )

            # An explicit native source file is an additional source in V2,
            # not a replacement selector. Record this so an aix profile launch
            # can clear/reject inherited explicit sources before adding its own
            # ChatGPT runtime content.
            explicit_source = root / "explicit-source.json"
            json_file(explicit_source, {"username": "explicit-source-is-an-overlay"})
            env_explicit = dict(env_a)
            env_explicit.pop("OPENCODE_CONFIG_CONTENT", None)
            env_explicit["OPENCODE_CONFIG"] = str(explicit_source)
            explicit_entries = config_entries(binary, project, env_explicit)
            require(
                any(
                    doc.get("info", {}).get("username")
                    == "explicit-source-is-an-overlay"
                    for doc in documents(explicit_entries)
                ),
                "the v2 OPENCODE_CONFIG source behavior changed; revisit selector precedence",
            )

            run_result = invoke(
                [
                    binary,
                    "run",
                    "--standalone",
                    "--model",
                    "fixture/explicit",
                    "--format",
                    "json",
                    "--title",
                    "issue33-history-fixture",
                    "Report the project instruction marker.",
                ],
                cwd=project,
                env=env_a,
            )
            require(
                "fixture response" in run_result.stdout,
                "local fixture model did not complete the run",
            )
            requests = model_server.requests  # type: ignore[attr-defined]
            require(
                requests,
                "selected API-key provider did not contact the loopback endpoint",
            )
            request = requests[-1]
            require(
                request["authorization"] == f"Bearer {API_FIXTURE}",
                "profile API key did not reach its endpoint",
            )
            require(
                request["body"].get("model") == "explicit",
                "ordinary --model override was not preserved",
            )
            prompt_text = "\n".join(all_strings(request["body"]))
            require(
                PROJECT_MARKER in prompt_text,
                "project AGENTS.md instructions were not sent to the provider",
            )

            # Exercise the existing aix ChatGPT provider shape end to end.
            # The fake local endpoint receives only the bridge-style bearer;
            # no real OAuth credential is read or used.
            bridge_run = invoke(
                [
                    binary,
                    "run",
                    "--standalone",
                    "--model",
                    f"aix-chatgpt/{BRIDGE_MODEL}",
                    "--format",
                    "json",
                    "--title",
                    "issue33-chatgpt-bridge-fixture",
                    "Exercise the ChatGPT bridge provider.",
                ],
                cwd=project,
                env=env_a,
            )
            require(
                "fixture response" in bridge_run.stdout,
                "the ChatGPT bridge provider did not complete the run",
            )
            bridge_requests = [
                item
                for item in model_server.requests  # type: ignore[attr-defined]
                if item["path"].rstrip("/").endswith("/responses")
            ]
            require(
                bridge_requests,
                "ChatGPT provider did not call the configured Responses endpoint",
            )
            bridge_request = bridge_requests[-1]
            require(
                bridge_request["authorization"] == f"Bearer {BRIDGE_FIXTURE}",
                "ChatGPT provider did not use the selected aix bridge bearer",
            )
            require(
                bridge_request["body"].get("model") == BRIDGE_MODEL,
                "ChatGPT bridge launch did not retain its selected model connection",
            )

            # Profile B selects a custom cli.json independently while leaving
            # the standard app config as its application source.
            env_b = dict(base_env)
            env_b["OPENCODE_CONFIG_DIR"] = str(profile_b)
            paths_b = path_map(binary, project, env_b)
            require(
                Path(paths_b["config"]) == profile_b,
                "profile B did not select its staged config root",
            )
            require(
                paths_b["data"] == paths_a["data"], "profile switch moved user data"
            )
            cli_b_value = json.loads(
                (profile_b / "cli.json").read_text(encoding="utf-8")
            )
            require(
                cli_b_value.get("animations") is True,
                "custom terminal config was not staged",
            )
            require(
                "mouse" not in cli_b_value,
                "standard terminal-only setting leaked into custom cli.json",
            )
            entries_b = config_entries(binary, project, env_b)
            docs_b = documents(entries_b)
            require(
                any(
                    doc.get("info", {}).get("username") == GLOBAL_MARKER
                    for doc in docs_b
                ),
                "terminal-only selection unexpectedly replaced the standard app config",
            )

            # The plugin-removal command is an observable CLI-config reader
            # and writer. Run it on a disposable staged clone: a global-only
            # CLI setting must be absent, while the selected setting is found
            # and its path is the selected cli.json, not the standard one.
            probe_env = dict(env_b)
            probe_env["OPENCODE_CONFIG_DIR"] = str(profile_b_cli_probe)
            global_cli_probe = invoke(
                [binary, "plugin", "remove", GLOBAL_CLI_PLUGIN],
                cwd=project,
                env=probe_env,
            )
            require(
                "is not configured" in global_cli_probe.stdout,
                "global-only CLI plugin setting leaked into profile B",
            )
            selected_cli_probe = invoke(
                [binary, "plugin", "remove", PROFILE_CLI_PLUGIN],
                cwd=project,
                env=probe_env,
            )
            require(
                str(profile_b_cli_probe / "cli.json") in selected_cli_probe.stdout,
                "CLI config command did not read and update the selected profile cli.json",
            )
            require(
                PROFILE_CLI_PLUGIN
                not in (profile_b_cli_probe / "cli.json").read_text(encoding="utf-8"),
                "CLI config probe did not remove its disposable marker",
            )

            # A private launch and explicit server selection are mutually
            # exclusive in V2; ordinary overrides such as --model remain valid.
            server_conflict = invoke(
                [
                    binary,
                    "api",
                    "--standalone",
                    "--server",
                    "http://127.0.0.1:1",
                    "GET",
                    "/config",
                ],
                cwd=project,
                env=env_a,
                expect=1,
            )
            require(
                "--server and --standalone cannot be combined"
                in server_conflict.stderr,
                "explicit --server conflict was not rejected by the V2 CLI",
            )

            # Existing synthetic session history and saved auth remain in the
            # unchanged data root after switching to a different config pair.
            sessions = json.loads(
                invoke(
                    [binary, "session", "list", "--standalone", "--format", "json"],
                    cwd=project,
                    env=env_b,
                ).stdout
            )
            matching = [
                item
                for item in sessions
                if item.get("title") == "issue33-history-fixture"
            ]
            require(
                matching,
                "the existing session disappeared after the profile config switch",
            )
            exported = json.loads(
                invoke(
                    [binary, "session", "export", "--standalone", matching[0]["id"]],
                    cwd=project,
                    env=env_b,
                ).stdout
            )
            require(
                any(
                    item.get("type") == "user"
                    and "project instruction marker" in item.get("text", "")
                    for item in exported["messages"]
                ),
                "session history was not available after the profile config switch",
            )
            auth_output = invoke(
                [binary, "auth", "list", "--standalone", "--format", "json"],
                cwd=project,
                env=env_b,
            ).stdout
            auth_entries = json.loads(auth_output)
            require(
                any(
                    connection.get("label") == "issue33-dummy-auth"
                    for item in auth_entries
                    for connection in item.get("connections", [])
                ),
                "saved application auth was not available after the profile config switch",
            )
            require(
                AUTH_FIXTURE not in auth_output,
                "auth list exposed the dummy saved-auth key",
            )

            # Two simultaneous private servers use different profile roots and
            # the same unchanged data root. Neither app config may cross over.
            env_c = dict(base_env)
            env_c["OPENCODE_CONFIG_DIR"] = str(profile_c)
            from concurrent.futures import ThreadPoolExecutor

            with ThreadPoolExecutor(max_workers=2) as pool:
                future_a = pool.submit(config_entries, binary, project, env_a)
                future_c = pool.submit(config_entries, binary, project, env_c)
                concurrent_a = documents(future_a.result())
                concurrent_c = documents(future_c.result())
            concurrent_a_json = json.dumps(concurrent_a)
            concurrent_c_json = json.dumps(concurrent_c)
            require(
                "profile-a-relative-resource" in concurrent_a_json,
                "concurrent profile A config crossed over",
            )
            require(
                "profile-c-only" not in concurrent_a_json,
                "profile C config leaked into profile A",
            )
            require(
                "profile-c-only" in concurrent_c_json,
                "concurrent profile C config crossed over",
            )
            require(
                "profile-a-relative-resource" not in concurrent_c_json,
                "profile A config leaked into profile C",
            )

            require(
                source_fingerprints == fingerprint(source_paths),
                "a standard, project, staged, or profile config source was modified",
            )

            print(
                "OpenCode V2 profile proof passed: replacement, independent app/CLI staging, project config/instructions, "
                "private server, preserved session history/auth, relative resources, bridge overlay, selector precedence, "
                "and concurrent profiles."
            )
            print(
                f"OpenCode boundary: {version}; test state was isolated under {scratch_parent}."
            )
        finally:
            model_server.shutdown()
            model_server.server_close()
            server_thread.join(timeout=2)


if __name__ == "__main__":
    main()
