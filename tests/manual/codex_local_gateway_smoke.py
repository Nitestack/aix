#!/usr/bin/env python3
"""Live Codex app-server/local-gateway smoke test; credentials stay in aix's auth store."""

from __future__ import annotations

import argparse
import json
import os
import select
import shlex
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import tomllib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


ALLOWED_CONNECTS = {"api.openai.com:443", "auth.openai.com:443"}
TOOL_NAME = "aix_smoke_echo"
TOOL_INPUT = "AIX_LOCAL_TOOL_SMOKE_INPUT"
TOOL_RESULT = "AIX_LOCAL_TOOL_SMOKE_OK"


class RecordingProxy(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self) -> None:
        super().__init__(("127.0.0.1", 0), ProxyHandler)
        self.targets: set[str] = set()
        self.blocked: set[str] = set()
        self.lock = threading.Lock()


class ProxyHandler(BaseHTTPRequestHandler):
    server: RecordingProxy

    def log_message(self, _format: str, *_args: object) -> None:
        pass

    def do_CONNECT(self) -> None:
        target = self.path.lower()
        with self.server.lock:
            self.server.targets.add(target)
        if target not in ALLOWED_CONNECTS:
            with self.server.lock:
                self.server.blocked.add(target)
            self.send_error(403)
            return

        host, separator, port = target.rpartition(":")
        if not separator:
            self.send_error(400)
            return
        try:
            upstream = socket.create_connection((host, int(port)), timeout=20)
        except OSError:
            self.send_error(502)
            return

        self.send_response(200, "Connection Established")
        self.end_headers()
        try:
            relay(self.connection, upstream)
        finally:
            upstream.close()

    def do_GET(self) -> None:
        self._reject_non_tunnel()

    def do_POST(self) -> None:
        self._reject_non_tunnel()

    def _reject_non_tunnel(self) -> None:
        with self.server.lock:
            self.server.blocked.add("non-CONNECT")
        self.send_error(403)


def relay(client: socket.socket, upstream: socket.socket) -> None:
    sockets = [client, upstream]
    while True:
        readable, _, _ = select.select(sockets, [], [])
        for source in readable:
            try:
                data = source.recv(64 * 1024)
            except OSError:
                return
            if not data:
                return
            destination = upstream if source is client else client
            try:
                destination.sendall(data)
            except OSError:
                return


class AppServerClient:
    def __init__(self, process: subprocess.Popen[bytes]) -> None:
        assert process.stdout is not None
        assert process.stdin is not None
        self.process = process
        self.stdout_fd = process.stdout.fileno()
        self.stdin = process.stdin
        self.buffer = bytearray()
        self.next_id = 1
        self.tool_called = False

    def send(self, message: dict[str, object]) -> None:
        self.stdin.write(json.dumps(message, separators=(",", ":")).encode() + b"\n")
        self.stdin.flush()

    def receive(self, timeout: float = 300) -> dict[str, object]:
        deadline = time.monotonic() + timeout
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError("timed out waiting for Codex app-server")
            readable, _, _ = select.select([self.stdout_fd], [], [], remaining)
            if not readable:
                raise RuntimeError("timed out waiting for Codex app-server")
            chunk = os.read(self.stdout_fd, 64 * 1024)
            if not chunk:
                raise RuntimeError("Codex app-server closed its protocol stream")
            self.buffer.extend(chunk)
            if len(self.buffer) > 16 * 1024 * 1024:
                raise RuntimeError(
                    "Codex app-server protocol message exceeded the smoke-test bound"
                )
        line, _, remaining = self.buffer.partition(b"\n")
        self.buffer = bytearray(remaining)
        try:
            message = json.loads(line)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise RuntimeError("Codex app-server emitted invalid JSON") from error
        if not isinstance(message, dict):
            raise RuntimeError("Codex app-server emitted a non-object message")
        return message

    def request(self, method: str, params: dict[str, object]) -> dict[str, object]:
        request_id = self.next_id
        self.next_id += 1
        self.send({"id": request_id, "method": method, "params": params})
        while True:
            message = self.receive()
            if message.get("id") == request_id:
                if "error" in message:
                    raise RuntimeError(f"Codex app-server rejected {method}")
                result = message.get("result")
                if not isinstance(result, dict):
                    raise RuntimeError(
                        f"Codex app-server returned an invalid {method} result"
                    )
                return result
            self.handle_server_request(message)

    def handle_server_request(self, message: dict[str, object]) -> None:
        if message.get("method") != "item/tool/call" or "id" not in message:
            return
        params = message.get("params")
        if not isinstance(params, dict) or params.get("tool") != TOOL_NAME:
            raise RuntimeError("Codex requested an unexpected dynamic tool")
        arguments = params.get("arguments")
        if isinstance(arguments, str):
            try:
                arguments = json.loads(arguments)
            except json.JSONDecodeError as error:
                raise RuntimeError(
                    "Codex supplied invalid dynamic-tool arguments"
                ) from error
        if not isinstance(arguments, dict) or arguments.get("text") != TOOL_INPUT:
            raise RuntimeError("Codex called the smoke tool with unexpected arguments")
        self.tool_called = True
        self.send(
            {
                "id": message["id"],
                "result": {
                    "success": True,
                    "contentItems": [{"type": "inputText", "text": TOOL_RESULT}],
                },
            }
        )

    def run_tool_turn(self, model: str) -> None:
        self.request(
            "initialize",
            {
                "clientInfo": {"name": "aix-local-gateway-smoke", "version": "1.0"},
                "capabilities": {"experimentalApi": True},
            },
        )
        self.send({"method": "initialized", "params": {}})
        thread = self.request(
            "thread/start",
            {
                "cwd": os.getcwd(),
                "model": model,
                "modelProvider": "aix_chatgpt_plan",
                "approvalPolicy": "never",
                "dynamicTools": [
                    {
                        "type": "function",
                        "name": TOOL_NAME,
                        "description": "Return the supplied text unchanged.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"text": {"type": "string"}},
                            "required": ["text"],
                            "additionalProperties": False,
                        },
                    }
                ],
            },
        )
        thread_info = thread.get("thread")
        if not isinstance(thread_info, dict) or not isinstance(
            thread_info.get("id"), str
        ):
            raise RuntimeError("Codex app-server did not return a thread ID")
        turn = self.request(
            "turn/start",
            {
                "threadId": thread_info["id"],
                "input": [
                    {
                        "type": "text",
                        "text": (
                            f"Call {TOOL_NAME} exactly once with text={TOOL_INPUT!r}. "
                            "Use the returned value in your final answer."
                        ),
                    }
                ],
            },
        )
        turn_info = turn.get("turn")
        turn_id = turn_info.get("id") if isinstance(turn_info, dict) else None
        if not isinstance(turn_id, str):
            raise RuntimeError("Codex app-server did not return a turn ID")

        while True:
            message = self.receive()
            if message.get("method") == "turn/completed":
                params = message.get("params")
                completed = params.get("turn") if isinstance(params, dict) else None
                if not isinstance(completed, dict) or completed.get("id") != turn_id:
                    continue
                if completed.get("status") != "completed":
                    raise RuntimeError(
                        "Codex app-server did not complete the smoke turn"
                    )
                break
            self.handle_server_request(message)

        if not self.tool_called:
            raise RuntimeError("Codex completed without calling the local smoke tool")


def validate_config(path: Path, profile: str, policy: str, model: str) -> None:
    with path.open("rb") as file:
        config = tomllib.load(file)
    profile_config = config.get("profiles", {}).get(profile, {})
    tool = config.get("tools", {}).get("codex", {})
    binding = tool.get("chatgpt", {})
    run_policy = config.get("run_policies", {}).get(policy, {})
    if profile_config.get("auth", {}).get("type") != "chatgpt":
        raise ValueError("selected profile must use ChatGPT auth")
    if (
        tool.get("api_format") != "openai"
        or binding.get("transport") != "local_gateway"
    ):
        raise ValueError(
            "[tools.codex] must use api_format = openai and transport = local_gateway"
        )
    if binding.get("access_token_env") != "ACCESS_TOKEN":
        raise ValueError('the smoke wrapper expects access_token_env = "ACCESS_TOKEN"')
    if tool.get("command", "codex") != "codex":
        raise ValueError(
            'the smoke wrapper expects command = "codex" or an omitted command'
        )
    if binding.get("prepend_args", [])[:3] != ["app-server", "--listen", "stdio://"]:
        raise ValueError(
            "Codex prepend_args must start with app-server --listen stdio://"
        )
    cleared = set(binding.get("clear_env", []))
    if not {"OPENAI_API_KEY", "CODEX_API_KEY"}.issubset(cleared):
        raise ValueError("clear_env must include OPENAI_API_KEY and CODEX_API_KEY")
    if run_policy.get("profile", profile) != profile:
        raise ValueError("the selected policy is fixed to a different profile")
    if run_policy.get("max_budget") is not None or model not in run_policy.get(
        "allowed_models", []
    ):
        raise ValueError(
            "the selected no-budget policy must allow the requested model ID"
        )


def read_usage_events(state_dir: Path) -> list[dict[str, object]]:
    events_dir = state_dir / "usage" / "events"
    events = []
    if events_dir.exists():
        for path in events_dir.glob("*/*.json"):
            try:
                value = json.loads(path.read_text())
            except (OSError, json.JSONDecodeError):
                continue
            if isinstance(value, dict):
                events.append(value)
    return events


def read_access_token_expiry(
    aix: str, config: Path, profile: str, environment: dict[str, str]
) -> int | None:
    result = subprocess.run(
        [
            aix,
            "--config",
            str(config),
            "--profile",
            profile,
            "auth",
            "status",
            profile,
            "--json",
        ],
        capture_output=True,
        text=True,
        env=environment,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError("could not read redacted aix auth status")
    try:
        envelope = json.loads(result.stdout)
        expiry = envelope["data"]["access_token_expires_at"]
    except (KeyError, TypeError, json.JSONDecodeError) as error:
        raise RuntimeError(
            "aix auth status returned an unexpected JSON envelope"
        ) from error
    return expiry if isinstance(expiry, int) else None


def make_child_wrapper(directory: Path, real_codex: str) -> tuple[Path, Path]:
    wrapper_dir = directory / "bin"
    wrapper_dir.mkdir()
    wrapper = wrapper_dir / "codex"
    report = directory / "child-env-verified"
    wrapper.write_text(
        "#!/bin/sh\n"
        "set -eu\n"
        'test -n "${ACCESS_TOKEN:-}"\n'
        'test "${#ACCESS_TOKEN}" -eq 64\n'
        'case "$ACCESS_TOKEN" in *[!0123456789abcdef]*) exit 81 ;; esac\n'
        'test -z "${OPENAI_API_KEY+x}"\n'
        'test -z "${CODEX_API_KEY+x}"\n'
        'test -n "${CODEX_HOME:-}" && test ! -e "$CODEX_HOME/auth.json"\n'
        "printf 'ok\\n' > \"$AIX_SMOKE_CHILD_ENV_REPORT\"\n"
        "printf '%s\\n' \"$@\" | grep -Eq 'model_providers\\.aix_chatgpt_plan\\.base_url=\"http://127\\.0\\.0\\.1:[0-9]+/v1\"'\n"
        f'exec {shlex.quote(real_codex)} "$@"\n'
    )
    wrapper.chmod(0o700)
    return wrapper_dir, report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True, type=Path)
    parser.add_argument("--profile", required=True)
    parser.add_argument("--policy", required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--aix", default="aix")
    parser.add_argument("--require-refresh", action="store_true")
    args = parser.parse_args()

    validate_config(args.config, args.profile, args.policy, args.model)
    aix = shutil.which(args.aix)
    real_codex = shutil.which("codex")
    if not aix or not real_codex:
        raise SystemExit("aix and codex must both be available on PATH")

    with tempfile.TemporaryDirectory(prefix="aix-codex-smoke-") as temporary:
        temp = Path(temporary)
        codex_home = temp / "empty-codex-home"
        codex_home.mkdir(mode=0o700)
        state_dir = temp / "aix-state"
        wrapper_dir, child_report = make_child_wrapper(temp, real_codex)
        proxy = RecordingProxy()
        proxy_thread = threading.Thread(target=proxy.serve_forever, daemon=True)
        proxy_thread.start()

        environment = os.environ.copy()
        environment.update(
            {
                "AIX_STATE_DIR": str(state_dir),
                "AIX_SMOKE_CHILD_ENV_REPORT": str(child_report),
                "CODEX_HOME": str(codex_home),
                "PATH": str(wrapper_dir) + os.pathsep + environment.get("PATH", ""),
            }
        )
        proxy_url = f"http://127.0.0.1:{proxy.server_port}"
        for name in (
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ):
            environment[name] = proxy_url
        environment["NO_PROXY"] = "localhost,127.0.0.1,::1"
        environment["no_proxy"] = environment["NO_PROXY"]
        access_token_expiry_before = read_access_token_expiry(
            aix, args.config, args.profile, environment
        )

        process = subprocess.Popen(
            [
                aix,
                "--config",
                str(args.config),
                "--profile",
                args.profile,
                "run",
                "--policy",
                args.policy,
                "--",
                "codex",
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            env=environment,
        )
        client = AppServerClient(process)
        try:
            client.run_tool_turn(args.model)
            assert process.stdin is not None
            process.stdin.close()
            exit_code = process.wait(timeout=30)
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            proxy.shutdown()
            proxy.server_close()
            proxy_thread.join(timeout=2)

        if exit_code != 0:
            raise RuntimeError("aix/Codex smoke process failed")
        if not child_report.exists() or child_report.read_text().strip() != "ok":
            raise RuntimeError(
                "the Codex child did not satisfy the local-bearer environment checks"
            )
        if (codex_home / "auth.json").exists():
            raise RuntimeError(
                "Codex wrote auth.json despite the isolated empty CODEX_HOME"
            )
        access_token_expiry_after = read_access_token_expiry(
            aix, args.config, args.profile, environment
        )
        refresh_seen = (
            access_token_expiry_before is not None
            and access_token_expiry_after is not None
            and access_token_expiry_after > access_token_expiry_before
        )

        events = [
            event
            for event in read_usage_events(state_dir)
            if event.get("protocol") == "openai_responses"
            and event.get("logical_tool_name") == "codex"
        ]
        event_summaries = [
            {
                key: event.get(key)
                for key in (
                    "model",
                    "run_policy",
                    "outcome",
                    "error_category",
                    "usage_completeness",
                    "input_tokens_total",
                    "output_tokens",
                )
            }
            for event in events
        ]
        if not any(
            event.get("outcome") == "succeeded"
            and event.get("model") == args.model
            and event.get("run_id")
            and event.get("run_policy") == args.policy
            and event.get("input_tokens_total") is not None
            and event.get("output_tokens") is not None
            for event in events
        ):
            raise RuntimeError(
                "no successful attributed local Responses usage event was recorded; "
                "safe event summaries=" + json.dumps(event_summaries, sort_keys=True)
            )

        targets = set(proxy.targets)
        unexpected_targets = targets.difference(ALLOWED_CONNECTS)
        if proxy.blocked or unexpected_targets:
            raise RuntimeError(
                "external traffic did not match the OpenAI-only proxy allowlist; "
                f"observed CONNECT hosts={sorted(targets)}, blocked={sorted(proxy.blocked)}"
            )
        proxy_observation = (
            "verified" if "api.openai.com:443" in targets else "not_observed"
        )
        if args.require_refresh and not refresh_seen:
            raise RuntimeError(
                "the stored access-token expiry did not advance; refresh was not verified"
            )

        print("inference=passed; codex_login=not_required; child_local_bearer=verified")
        print("dynamic_local_tool=passed; attributed_usage=recorded")
        print("expected_inference_connect=" + proxy_observation)
        print("external_connects=" + ",".join(sorted(targets)))
        print(
            "oauth_refresh_proxy_observation="
            + ("observed" if refresh_seen else "not_observed")
        )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, ValueError) as error:
        raise SystemExit(f"Codex local-gateway smoke failed: {error}") from error
