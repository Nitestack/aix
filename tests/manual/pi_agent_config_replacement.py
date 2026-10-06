#!/usr/bin/env python3
"""Offline Pi/aix profile-config replacement proof using synthetic credentials."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import tempfile
import threading
from pathlib import Path

from pi_agent_config_replacement_support import (
    PI_VERSION,
    PROFILE_A_KEY,
    PROFILE_B_KEY,
    MockOpenAIServer,
    ProofContext,
    create_normal_agent,
    make_aix_config,
    native_project_session_dir,
    profile_config,
    require,
    run_process,
    write_extension,
    write_json,
)
from pi_agent_config_replacement_checks import (
    verify_profile_configuration,
    verify_sessions_and_launches,
)


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
        trust_path = normal_agent / "trust.json"
        write_json(trust_path, {str(project.resolve()): True})
        trust_path.chmod(0o444)
        for agent in (profile_a_agent, profile_b_agent):
            (agent / "trust.json").symlink_to(trust_path)

        wrapper = Path(__file__).with_name("pi_agent_config_overlay.py").resolve()
        shim_dir = root / "pi-shim"
        shim_dir.mkdir()
        (shim_dir / "pi").symlink_to(wrapper)
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
            shim_path=f"{shim_dir}{os.pathsep}{os.environ.get('PATH', '')}",
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
                "SHELL": "/bin/sh",
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

        proof = ProofContext(
            aix=aix,
            pi=pi,
            pi_package=pi_package,
            root=root,
            project=project,
            normal_agent=normal_agent,
            normal_session_dir=normal_session_dir,
            profile_a_agent=profile_a_agent,
            profile_b_agent=profile_b_agent,
            profile_a_server=profile_a_server,
            profile_b_server=profile_b_server,
            profile_a_url=profile_a_url,
            profile_b_url=profile_b_url,
            stage_root=stage_root,
            wrapper=wrapper,
            aix_config=aix_config,
            env=env,
        )
        source_hashes = verify_profile_configuration(proof)
        verify_sessions_and_launches(proof, source_hashes)

        print(f"pi_version={version}")
        print("config_replacement=settings+models+keybindings+resources=passed")
        print("project_rules_and_trust=passed")
        print("session_history_and_saved_auth=passed")
        print("aix_shell_run_named_tool_exec_and_concurrent_profiles=passed")
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
