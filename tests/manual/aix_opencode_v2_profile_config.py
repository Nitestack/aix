#!/usr/bin/env python3
"""Exercise aix's OpenCode profile staging against the pinned OpenCode v2 CLI.

Requires Python 3.10+, aix, and OpenCode v2.0.20. All OpenCode homes, config,
data, cache, and state are isolated under /tmp/opencode; no live inference or
external model endpoint is used.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import threading
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any


VERSION = "opencode v2.0.20"
SELECTED_APP = "AIX_ISSUE34_SELECTED_APP"
STANDARD_APP = "AIX_ISSUE34_STANDARD_APP_MUST_NOT_LEAK"
INHERITED_APP = "AIX_ISSUE34_INHERITED_APP_MUST_NOT_LEAK"
EXPLICIT_APP = "AIX_ISSUE34_EXPLICIT_APP_MUST_NOT_LEAK"
INLINE_APP = "AIX_ISSUE34_INLINE_APP_MUST_NOT_LEAK"
STANDARD_PLUGIN = "aix-issue34-standard-cli-plugin"
INHERITED_PLUGIN = "aix-issue34-inherited-cli-plugin"
PROFILE_PLUGIN = "aix-issue34-profile-cli-plugin"
MODEL_API_KEY = "ISSUE34_DUMMY_MODEL_KEY_NEVER_PRINT"
SAVED_AUTH_KEY = "ISSUE34_DUMMY_SAVED_AUTH_KEY_NEVER_PRINT"
PROJECT_INSTRUCTION = "AIX_ISSUE34_PROJECT_INSTRUCTION_MARKER"


class FakeModelHandler(BaseHTTPRequestHandler):
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
            chunks = [
                {
                    "id": "chatcmpl-issue34",
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
                    "id": "chatcmpl-issue34",
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
                "id": "chatcmpl-issue34",
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


def resolve_executable(name: str, default: Path) -> str:
    candidate = os.environ.get(name)
    if candidate is None:
        candidate = str(default)
    resolved = shutil.which(candidate) or (
        candidate if Path(candidate).is_file() else None
    )
    if resolved is None:
        raise RuntimeError(f"Set {name} to an executable path; not found: {candidate}")
    return str(Path(resolved).resolve())


def invoke(
    command: list[str], *, cwd: Path, env: dict[str, str]
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
    for secret in (
        "ISSUE34_DUMMY_API_KEY_NEVER_PRINT",
        MODEL_API_KEY,
        SAVED_AUTH_KEY,
    ):
        if secret in output:
            raise AssertionError(f"OpenCode printed a fixture credential: {secret}")
    if result.returncode != 0:
        raise RuntimeError(f"command exited {result.returncode}: {command!r}\n{output}")
    return result


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def toml_string(value: str) -> str:
    return json.dumps(value)


def config_documents(output: str) -> list[dict[str, Any]]:
    entries = json.loads(output)
    return [entry for entry in entries if entry.get("type") == "document"]


def all_strings(value: Any) -> list[str]:
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        return [item for child in value for item in all_strings(child)]
    if isinstance(value, dict):
        return [item for child in value.values() for item in all_strings(child)]
    return []


def path_map(output: str) -> dict[str, str]:
    return {
        key.strip(): value.strip()
        for key, value in (
            line.split(None, 1) for line in output.splitlines() if line.strip()
        )
    }


def trace_files(directory: Path) -> list[dict[str, Any]]:
    traces = [
        json.loads(path.read_text(encoding="utf-8"))
        for path in directory.glob("*.json")
    ]
    require(traces, f"aix did not invoke the OpenCode wrapper in {directory}")
    return sorted(traces, key=lambda trace: trace["created_at"])


def assert_stage(
    trace: dict[str, Any], marker: str, *, private_server: bool = True
) -> Path:
    arguments = trace["argv"]
    require(
        ("--standalone" in arguments) == private_server,
        f"unexpected private-server setting for OpenCode args {arguments}",
    )
    if private_server:
        if arguments[0] in ("run", "api", "models"):
            require(
                arguments[1] == "--standalone", f"misplaced --standalone: {arguments}"
            )
        elif arguments[0] in ("session", "auth"):
            require(
                arguments[2] == "--standalone", f"misplaced --standalone: {arguments}"
            )
        else:
            require(
                arguments[0] == "--standalone", f"misplaced --standalone: {arguments}"
            )
    stage = Path(trace["config_dir"])
    require(
        stage.name.startswith("aix-opencode-config-"),
        f"unexpected staged config dir: {stage}",
    )
    require(marker in trace["app_config"], f"staged app config omitted {marker}")
    return stage


def main() -> None:
    repo = Path(__file__).resolve().parents[2]
    aix = resolve_executable("AIX_BIN", repo / "target/debug/aix")
    opencode = resolve_executable("OPENCODE_BIN", Path("opencode"))
    version = subprocess.run(
        [opencode, "--version"], check=True, text=True, capture_output=True
    ).stdout.strip()
    require(version == VERSION, f"test boundary is {VERSION!r}, found {version!r}")

    scratch_parent = Path("/tmp/opencode")
    if not scratch_parent.is_dir():
        raise RuntimeError(
            "Create /tmp/opencode first; scratch data stays under that directory"
        )

    with tempfile.TemporaryDirectory(
        prefix="aix-issue-34-", dir=scratch_parent
    ) as temporary:
        root = Path(temporary)
        home = root / "home"
        xdg_config = root / "xdg-config"
        standard = xdg_config / "opencode"
        inherited = root / "inherited-config"
        project = root / "project"
        app_source = root / "profile-app"
        cli_source = root / "profile-cli"
        traces = root / "traces"
        for directory in (
            home,
            standard,
            inherited,
            project,
            app_source,
            cli_source,
            traces,
        ):
            directory.mkdir(parents=True, exist_ok=True)

        model_server = ThreadingHTTPServer(("127.0.0.1", 0), FakeModelHandler)
        model_server.requests = []  # type: ignore[attr-defined]
        model_thread = threading.Thread(target=model_server.serve_forever, daemon=True)
        model_thread.start()
        model_base_url = f"http://127.0.0.1:{model_server.server_port}/v1"

        standard_app = standard / "opencode.json"
        standard_cli = standard / "cli.json"
        inherited_app = inherited / "opencode.json"
        inherited_cli = inherited / "cli.json"
        selected_app = app_source / "opencode.json"
        selected_resource = app_source / "resources" / "app-marker.txt"
        selected_cli = cli_source / "cli.json"
        explicit_app = root / "explicit-opencode.json"
        project_config = project / "opencode.json"

        write_json(
            standard_app,
            {"$schema": "https://opencode.ai/config.json", "username": STANDARD_APP},
        )
        write_json(
            standard_cli,
            {
                "$schema": "https://opencode.ai/v2/cli.json",
                "animations": False,
                "plugins": [STANDARD_PLUGIN],
            },
        )
        write_json(
            inherited_app,
            {"$schema": "https://opencode.ai/config.json", "username": INHERITED_APP},
        )
        write_json(
            inherited_cli,
            {
                "$schema": "https://opencode.ai/v2/cli.json",
                "animations": False,
                "plugins": [INHERITED_PLUGIN],
            },
        )
        write_json(
            selected_app,
            {
                "$schema": "https://opencode.ai/config.json",
                "username": SELECTED_APP,
                "providers": {
                    "fixture": {
                        "name": "Issue #34 local model fixture",
                        "env": ["AIX_ISSUE34_API_KEY"],
                        "package": "@opencode/ai/providers/openai-compatible",
                        "settings": {"baseURL": model_base_url},
                        "models": {
                            "configured": {
                                "name": "Configured fixture model",
                                "limit": {"context": 8192, "output": 1024},
                            }
                        },
                    }
                },
            },
        )
        selected_resource.parent.mkdir(parents=True, exist_ok=True)
        selected_resource.write_text("relative profile resource\n", encoding="utf-8")
        write_json(
            selected_cli,
            {
                "$schema": "https://opencode.ai/v2/cli.json",
                "animations": True,
                "plugins": [PROFILE_PLUGIN],
            },
        )
        write_json(explicit_app, {"username": EXPLICIT_APP})
        write_json(
            project_config,
            {"$schema": "https://opencode.ai/config.json", "share": "manual"},
        )
        (project / "AGENTS.md").write_text(
            f"When asked for the local proof marker, include {PROJECT_INSTRUCTION}.\n",
            encoding="utf-8",
        )

        # This tiny shim records what aix actually hands to OpenCode, then execs
        # the pinned native binary unchanged.
        wrapper = root / "opencode-wrapper"
        wrapper.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, sys, time\n"
            "from pathlib import Path\n"
            "root = Path(os.environ['AIX_OPENCODE_TRACE_DIR'])\n"
            "config_dir = Path(os.environ['OPENCODE_CONFIG_DIR'])\n"
            "app = next((config_dir / name for name in ('opencode.json', 'opencode.jsonc') if (config_dir / name).is_file()), None)\n"
            "trace = {'argv': sys.argv[1:], 'config_dir': str(config_dir), 'app_config': app.read_text(encoding='utf-8') if app else '', 'cli_config': (config_dir / 'cli.json').read_text(encoding='utf-8') if (config_dir / 'cli.json').is_file() else '', 'selectors': {name: os.environ.get(name) for name in ('OPENCODE_CONFIG', 'OPENCODE_CONFIG_CONTENT', 'OPENCODE_CLI_CONFIG_CONTENT', 'OPENCODE_SERVER', 'OPENCODE_SERVER_URL')}}\n"
            "trace['created_at'] = time.time_ns()\n"
            "(root / f'{os.getpid()}-{time.time_ns()}.json').write_text(json.dumps(trace), encoding='utf-8')\n"
            "binary = os.environ['AIX_OPENCODE_REAL_BIN']\n"
            "os.execv(binary, [binary, *sys.argv[1:]])\n",
            encoding="utf-8",
        )
        wrapper.chmod(0o700)

        aix_config = root / "aix.toml"
        aix_config.write_text(
            "\n".join(
                [
                    "[endpoint]",
                    'base_url = "http://127.0.0.1:9/v1"',
                    "",
                    "[profiles.app]",
                    'api_key = "ISSUE34_DUMMY_API_KEY_NEVER_PRINT"',
                    "[profiles.app.tool_configs.opencode]",
                    f"config_file = {toml_string(str(selected_app))}",
                    "",
                    "[profiles.cli]",
                    'api_key = "ISSUE34_DUMMY_API_KEY_NEVER_PRINT"',
                    "[profiles.cli.tool_configs.opencode]",
                    f"cli_config_file = {toml_string(str(selected_cli))}",
                    "",
                    "[tools.opencode]",
                    f"command = {toml_string(str(wrapper))}",
                    'api_format = "openai"',
                    "",
                ]
            ),
            encoding="utf-8",
        )

        base_env = os.environ.copy()
        for name in (
            "AIX_CONFIG",
            "AIX_PROFILE",
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
            "AIX_ISSUE34_API_KEY",
        ):
            base_env.pop(name, None)
        base_env.update(
            {
                "HOME": str(home),
                "OPENCODE_TEST_HOME": str(home),
                "XDG_CONFIG_HOME": str(xdg_config),
                "XDG_DATA_HOME": str(root / "xdg-data"),
                "XDG_CACHE_HOME": str(root / "xdg-cache"),
                "XDG_STATE_HOME": str(root / "xdg-state"),
                "OPENCODE_DISABLE_MODELS_FETCH": "true",
                "OPENCODE_DISABLE_FILEWATCHER": "true",
                "AIX_ISSUE34_API_KEY": MODEL_API_KEY,
                "AIX_OPENCODE_REAL_BIN": opencode,
                "AIX_OPENCODE_TRACE_DIR": str(traces),
                "AIX_STATE_DIR": str(root / "aix-state"),
                "NO_PROXY": "127.0.0.1,localhost",
                "no_proxy": "127.0.0.1,localhost",
            }
        )

        auth_fixture = root / "auth-fixture.json"
        write_json(
            auth_fixture,
            [
                {
                    "id": "cred_issue34_fixture",
                    "integrationID": "openai",
                    "label": "issue34-dummy-auth",
                    "active": True,
                    "value": {"type": "key", "key": SAVED_AUTH_KEY},
                }
            ],
        )
        auth_fixture.chmod(0o600)
        invoke(
            [opencode, "auth", "import", "--standalone", str(auth_fixture)],
            cwd=project,
            env=base_env,
        )

        tracked_sources = [
            standard_app,
            standard_cli,
            inherited_app,
            inherited_cli,
            selected_app,
            selected_resource,
            selected_cli,
            explicit_app,
            project_config,
            project / "AGENTS.md",
            auth_fixture,
        ]
        source_fingerprints = {
            path: hashlib.sha256(path.read_bytes()).hexdigest()
            for path in tracked_sources
        }

        def aix_command(
            profile: str, tool_args: list[str], *, use_run: bool = False
        ) -> list[str]:
            common = [
                aix,
                "--config",
                str(aix_config),
                "--profile",
                profile,
                "--non-interactive",
            ]
            if use_run:
                return [*common, "run", "--", "opencode", *tool_args]
            return [*common, "opencode", "--", *tool_args]

        # A profile app config replaces the global app file; project config is
        # still discovered, and the ordinary global cli.json is retained.
        app_result = invoke(
            aix_command("app", ["api", "config.get"]), cwd=project, env=base_env
        )
        app_documents = config_documents(app_result.stdout)
        app_dump = json.dumps(app_documents)
        require(
            SELECTED_APP in app_dump,
            "OpenCode did not read the aix-selected app config",
        )
        require(
            STANDARD_APP not in app_dump,
            "standard app config leaked into selected profile",
        )
        require(
            any(
                document.get("info", {}).get("share") == "manual"
                for document in app_documents
            ),
            "project app config was not retained alongside the selected profile config",
        )
        app_trace = trace_files(traces)[-1]
        app_stage = assert_stage(app_trace, SELECTED_APP)
        require(
            json.loads(app_trace["cli_config"])
            == json.loads(standard_cli.read_text(encoding="utf-8")),
            "aix did not retain the standard cli.json beside the selected app config",
        )
        require(
            not app_stage.exists(), "aix did not clean up its temporary config stage"
        )

        model_run = invoke(
            aix_command(
                "app",
                [
                    "run",
                    "--model",
                    "fixture/configured",
                    "--format",
                    "json",
                    "--title",
                    "issue34-history-fixture",
                    "Send the local project instruction marker.",
                ],
            ),
            cwd=project,
            env=base_env,
        )
        require(
            "fixture response" in model_run.stdout,
            "loopback model did not complete the run",
        )
        model_requests = model_server.requests  # type: ignore[attr-defined]
        require(
            model_requests, "aix's real OpenCode launch did not call the loopback model"
        )
        request = model_requests[-1]
        require(
            request["authorization"] == f"Bearer {MODEL_API_KEY}",
            "selected profile provider did not use the loopback fixture key",
        )
        require(
            request["body"].get("model") == "configured",
            "OpenCode did not use the selected profile's configured fixture model",
        )
        require(
            PROJECT_INSTRUCTION in "\n".join(all_strings(request["body"])),
            "project AGENTS.md instructions were not sent through aix's OpenCode launch",
        )
        model_trace = trace_files(traces)[-1]
        model_stage = assert_stage(model_trace, SELECTED_APP)
        require(not model_stage.exists(), "aix did not clean up the inference stage")

        app_paths_output = invoke(
            aix_command("app", ["debug", "paths"]), cwd=project, env=base_env
        ).stdout
        app_paths = path_map(app_paths_output)
        require(
            Path(app_paths["data"]) == root / "xdg-data" / "opencode",
            "aix profile app selection changed OpenCode's data directory",
        )

        standard_plugin_result = invoke(
            aix_command("app", ["plugin", "remove", STANDARD_PLUGIN]),
            cwd=project,
            env=base_env,
        )
        standard_plugin_trace = trace_files(traces)[-1]
        standard_plugin_stage = assert_stage(
            standard_plugin_trace, SELECTED_APP, private_server=False
        )
        require(
            STANDARD_PLUGIN in standard_plugin_trace["cli_config"],
            "standard CLI plugin was not staged beside the selected app config",
        )
        require(
            str(standard_plugin_stage / "cli.json") in standard_plugin_result.stdout,
            "OpenCode did not read the retained standard cli.json from aix's stage",
        )
        require(
            not standard_plugin_stage.exists(),
            "aix did not clean up the standard-counterpart stage",
        )

        # Exercise inherited selector and additive app selectors. aix must
        # replace the config directory and clear explicit app overlays so they
        # cannot silently override the profile's selected global app file.
        conflict_env = dict(base_env)
        conflict_env.update(
            {
                "OPENCODE_CONFIG_DIR": str(inherited),
                "OPENCODE_CONFIG": str(explicit_app),
                "OPENCODE_CONFIG_CONTENT": json.dumps({"username": INLINE_APP}),
            }
        )
        selected_result = invoke(
            aix_command("app", ["api", "config.get"]), cwd=project, env=conflict_env
        )
        selected_dump = json.dumps(config_documents(selected_result.stdout))
        for marker in (
            SELECTED_APP,
            STANDARD_APP,
            INHERITED_APP,
            EXPLICIT_APP,
            INLINE_APP,
        ):
            if marker == SELECTED_APP:
                require(
                    marker in selected_dump,
                    "profile app config lost to inherited selectors",
                )
            else:
                require(
                    marker not in selected_dump,
                    f"conflicting app source leaked: {marker}",
                )
        conflict_trace = trace_files(traces)[-1]
        conflict_stage = assert_stage(conflict_trace, SELECTED_APP)
        require(
            all(
                value is None
                for key, value in conflict_trace["selectors"].items()
                if key != "OPENCODE_CLI_CONFIG_CONTENT"
            ),
            f"aix left a conflicting OpenCode selector active: {conflict_trace['selectors']}",
        )
        require(
            json.loads(conflict_trace["cli_config"])
            == json.loads(inherited_cli.read_text(encoding="utf-8")),
            "inherited global cli.json was not retained as the app-config counterpart",
        )
        require(
            not conflict_stage.exists(), "aix did not clean up the conflict-test stage"
        )

        # A CLI-only profile is exercised through `aix run`. The standard app
        # file remains active, while the selected native cli.json replaces the
        # inherited terminal config. OpenCode's plugin command reads and
        # updates only its disposable staged copy.
        cli_env = dict(base_env)
        cli_env["OPENCODE_CONFIG_DIR"] = str(inherited)
        cli_check = invoke(
            aix_command("cli", ["api", "config.get"], use_run=True),
            cwd=project,
            env=cli_env,
        )
        cli_dump = json.dumps(config_documents(cli_check.stdout))
        require(
            INHERITED_APP in cli_dump,
            "CLI-only selection replaced the standard app config",
        )
        cli_trace = trace_files(traces)[-1]
        cli_stage = assert_stage(cli_trace, INHERITED_APP)
        require(
            json.loads(cli_trace["cli_config"])
            == json.loads(selected_cli.read_text(encoding="utf-8")),
            "aix did not stage the profile-selected cli.json",
        )
        require(not cli_stage.exists(), "aix did not clean up the CLI-only stage")

        cli_paths_output = invoke(
            aix_command("cli", ["debug", "paths"]), cwd=project, env=cli_env
        ).stdout
        cli_paths = path_map(cli_paths_output)
        require(
            Path(cli_paths["data"]) == Path(app_paths["data"]),
            "switching aix OpenCode profiles changed the shared user-data directory",
        )

        session_list = json.loads(
            invoke(
                aix_command(
                    "cli", ["session", "list", "--format", "json"], use_run=True
                ),
                cwd=project,
                env=cli_env,
            ).stdout
        )
        matching_sessions = [
            item
            for item in session_list
            if item.get("title") == "issue34-history-fixture"
        ]
        require(
            matching_sessions,
            "session disappeared after switching the aix profile config",
        )
        exported = json.loads(
            invoke(
                aix_command(
                    "cli",
                    ["session", "export", matching_sessions[0]["id"]],
                    use_run=True,
                ),
                cwd=project,
                env=cli_env,
            ).stdout
        )
        require(
            any(
                message.get("type") == "user"
                and "local project instruction marker" in message.get("text", "")
                for message in exported["messages"]
            ),
            "session history was not available after switching the aix profile config",
        )

        auth_entries = json.loads(
            invoke(
                aix_command("cli", ["auth", "list", "--format", "json"], use_run=True),
                cwd=project,
                env=cli_env,
            ).stdout
        )
        require(
            any(
                connection.get("label") == "issue34-dummy-auth"
                for item in auth_entries
                for connection in item.get("connections", [])
            ),
            "saved OpenCode application auth was not retained after profile switching",
        )

        global_plugin_result = invoke(
            aix_command("cli", ["plugin", "remove", INHERITED_PLUGIN], use_run=True),
            cwd=project,
            env=cli_env,
        )
        require(
            "is not configured" in global_plugin_result.stdout,
            "inherited CLI plugin leaked into the profile-selected cli.json",
        )
        selected_plugin_result = invoke(
            aix_command("cli", ["plugin", "remove", PROFILE_PLUGIN], use_run=True),
            cwd=project,
            env=cli_env,
        )
        selected_plugin_trace = trace_files(traces)[-1]
        selected_cli_stage = assert_stage(
            selected_plugin_trace, INHERITED_APP, private_server=False
        )
        require(
            str(selected_cli_stage / "cli.json") in selected_plugin_result.stdout,
            "OpenCode did not read and update the aix-staged profile cli.json",
        )
        require(
            not selected_cli_stage.exists(),
            "aix did not clean up the plugin-test stage",
        )
        require(
            PROFILE_PLUGIN in selected_cli.read_text(encoding="utf-8"),
            "OpenCode modified the selected cli.json source instead of its staged copy",
        )

        after_fingerprints = {
            path: hashlib.sha256(path.read_bytes()).hexdigest()
            for path in tracked_sources
        }
        require(
            source_fingerprints == after_fingerprints,
            "a native or selected config source was modified",
        )
        model_server.shutdown()
        model_server.server_close()
        model_thread.join(timeout=2)
        print(
            "aix/OpenCode v2.0.20 profile proof passed: app replacement, retained standard counterparts, "
            "project instructions, CLI-only replacement through aix run, selector conflict handling, "
            "session/history/auth preservation, private staging, and source preservation."
        )
        print(
            f"OpenCode boundary: {version}; all fixture data was isolated under {scratch_parent}."
        )


if __name__ == "__main__":
    main()
