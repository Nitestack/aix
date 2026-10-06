"""Acceptance checks for the Pi agent-directory replacement proof."""

from __future__ import annotations

import concurrent.futures
import json
import subprocess
from functools import partial
from pathlib import Path

from pi_agent_config_replacement_support import (
    PROFILE_A_KEY,
    PROFILE_B_KEY,
    SAVED_ANTHROPIC_KEY,
    SAVED_AUTH_KEY,
    ProofContext,
    find_request,
    inspect_settings_and_keys,
    list_models,
    request_text,
    require,
    run_aix,
    run_process,
    run_pi_from_aix_shell,
    run_staged_pi,
    sha256,
    tool_names,
)


def verify_profile_configuration(proof: ProofContext) -> dict[Path, str]:
    aix = proof.aix
    aix_config = proof.aix_config
    profile_a_agent = proof.profile_a_agent
    profile_b_agent = proof.profile_b_agent
    normal_session_dir = proof.normal_session_dir
    project = proof.project
    env = proof.env
    pi = proof.pi
    normal_agent = proof.normal_agent
    stage_root = proof.stage_root
    profile_a_url = proof.profile_a_url
    pi_package = proof.pi_package
    wrapper = proof.wrapper
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
        and selected_env.get("PI_CODING_AGENT_SESSION_DIR") == str(normal_session_dir),
        f"aix did not select the profile config and shared session dirs: {selected_env}",
    )

    # Prove the default global directory and each selected profile are distinct.
    global_models = list_models(pi, normal_agent, project, "normal-only-model")
    selected_models = run_staged_pi(
        proof,
        source_agent=profile_a_agent,
        profile="profile_a",
        args=["--list-models", "profile-a-model"],
        api_key=PROFILE_A_KEY,
    ).stdout
    leaked_models = run_staged_pi(
        proof,
        source_agent=profile_a_agent,
        profile="profile_a",
        args=["--list-models", "normal-only-model"],
        api_key=PROFILE_A_KEY,
    ).stdout
    require("normal-only-model" in global_models, "normal fixture model was not found")
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
    trusted_project_settings = inspect_settings_and_keys(
        pi_package, profile_a_agent, project, project_trusted=True
    )
    untrusted_project_settings = inspect_settings_and_keys(
        pi_package, profile_a_agent, project, project_trusted=False
    )
    require(
        trusted_project_settings.get("thinking") == "low",
        "trusted project settings were not loaded",
    )
    require(
        untrusted_project_settings.get("thinking") != "low",
        "untrusted project settings were loaded",
    )

    auth_result = run_staged_pi(
        proof,
        source_agent=profile_a_agent,
        profile="profile_a",
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
    require(
        (profile_a_agent / "trust.json").resolve()
        == (normal_agent / "trust.json").resolve(),
        "profile A trust decisions are not shared with the original Pi home",
    )

    source_paths = (
        normal_agent / "settings.json",
        normal_agent / "models.json",
        normal_agent / "keybindings.json",
        normal_agent / "auth.json",
        normal_agent / "trust.json",
        normal_agent / "AGENTS.md",
        normal_agent / "extensions" / "global-only.js",
        profile_a_agent / "settings.json",
        profile_a_agent / "models.json",
        profile_a_agent / "keybindings.json",
        profile_a_agent / "AGENTS.md",
        profile_a_agent.parent / "shared" / "extensions" / "profile.js",
        profile_a_agent.parent / "shared" / "skills" / "profile-proof" / "SKILL.md",
        profile_b_agent / "settings.json",
        profile_b_agent / "models.json",
        profile_b_agent / "keybindings.json",
        profile_b_agent / "AGENTS.md",
        profile_b_agent.parent / "shared" / "extensions" / "profile.js",
        profile_b_agent.parent / "shared" / "skills" / "profile-proof" / "SKILL.md",
        project / "AGENTS.md",
        project / ".pi" / "APPEND_SYSTEM.md",
        project / ".pi" / "settings.json",
        project / ".pi" / "extensions" / "project-only.js",
    )
    return {path: sha256(path) for path in source_paths}


def verify_sessions_and_launches(
    proof: ProofContext, source_hashes: dict[Path, str]
) -> None:
    verify_session_preservation(proof)
    verify_launch_modes(proof)
    verify_native_provider_switch_blocker(proof)
    verify_concurrent_profiles(proof, source_hashes)
    verify_selector_guards(proof)


def verify_session_preservation(proof: ProofContext) -> None:
    profile_a_agent = proof.profile_a_agent
    normal_agent = proof.normal_agent
    normal_session_dir = proof.normal_session_dir
    profile_a_server = proof.profile_a_server
    project = proof.project
    env = proof.env
    pi = proof.pi
    run_aix_for_profile = partial(run_aix, proof)
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
        proof,
        source_agent=profile_a_agent,
        profile="profile_a",
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

    resumed = run_aix_for_profile(
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


def verify_launch_modes(proof: ProofContext) -> None:
    profile_a_server = proof.profile_a_server
    run_aix_for_profile = partial(run_aix, proof)
    named = run_aix_for_profile(
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

    executed = run_aix_for_profile(
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

    shell_launch = run_pi_from_aix_shell(
        proof,
        "profile_a",
        "--model",
        "openai/profile-a-model",
        "--print",
        "--approve",
        "profile-a-shell-marker",
    )
    require("reply:profile-a" in shell_launch.stdout, "aix shell launch failed")
    shell_request = find_request(profile_a_server, "profile-a-shell-marker")
    require(
        shell_request["authorization"] == f"Bearer {PROFILE_A_KEY}",
        "aix shell did not keep the profile connection authoritative",
    )


def verify_native_provider_switch_blocker(proof: ProofContext) -> None:
    profile_a_agent = proof.profile_a_agent
    auth_env = proof.env.copy()
    auth_env["PI_CODING_AGENT_DIR"] = str(profile_a_agent)
    auth_env.pop("PI_CODING_AGENT_SESSION_DIR", None)
    run_aix_for_profile = partial(run_aix, proof)
    saved_auth = run_process(
        [proof.pi, "auth", "print-api-key", "--provider", "anthropic"],
        cwd=proof.project,
        env=auth_env,
    )
    require(
        saved_auth.stdout.strip() == SAVED_ANTHROPIC_KEY,
        "saved secondary-provider auth was not preserved",
    )

    switched = run_aix_for_profile(
        "profile_a",
        "--mode",
        "rpc",
        "--no-session",
        "--model",
        "openai/profile-a-model",
        mode="exec",
        input_text=(
            '{"id":"switch","type":"set_model",'
            '"provider":"anthropic","modelId":"saved-provider-model"}\n'
        ),
    )
    responses = [
        json.loads(line)
        for line in switched.stdout.splitlines()
        if line.startswith("{")
    ]
    response = next((item for item in responses if item.get("id") == "switch"), None)
    require(
        response is not None and response.get("success") is True,
        "expected Pi RPC set_model to reproduce the provider-switch blocker",
    )
    require(
        response["data"].get("provider") == "anthropic"
        and response["data"].get("baseUrl") == "http://saved-provider.invalid/v1",
        f"Pi RPC did not switch to the configured secondary endpoint: {response}",
    )


def verify_concurrent_profiles(
    proof: ProofContext, source_hashes: dict[Path, str]
) -> None:
    profile_a_server = proof.profile_a_server
    profile_b_server = proof.profile_b_server
    normal_agent = proof.normal_agent
    run_aix_for_profile = partial(run_aix, proof)
    # Different profiles launch at the same time, sharing the existing session
    # store but selecting different config roots, keys, and gateway endpoints.
    prompts = {
        "profile_a": "profile-a-concurrent-marker",
        "profile_b": "profile-b-concurrent-marker",
    }
    sessions_before_concurrent = list((normal_agent / "sessions").rglob("*.jsonl"))
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
        futures = {}
        for profile, prompt in prompts.items():
            pi_args = [
                "--model",
                f"openai/{profile.replace('_', '-')}-model",
                "--print",
            ]
            if profile == "profile_b":
                pi_args.append("--no-approve")
            pi_args.append(prompt)
            futures[profile] = executor.submit(run_aix_for_profile, profile, *pi_args)
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
    require("PROJECT_CONTEXT_MARKER" in a_text, "project instructions were not loaded")
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
    require("project_only_tool" in a_tools, "approved project extension was not loaded")
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
        all(sha256(path) == digest for path, digest in source_hashes.items()),
        "a Pi/AIX run modified a read-only configuration/auth source",
    )


def verify_selector_guards(proof: ProofContext) -> None:
    env = proof.env
    profile_a_agent = proof.profile_a_agent
    profile_a_url = proof.profile_a_url
    stage_root = proof.stage_root
    pi = proof.pi
    root = proof.root
    project = proof.project
    wrapper = proof.wrapper
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
        (["--extension", str(root / "other-provider.js")], "extension source"),
        (["-e", str(root / "other-provider.js")], "extension source"),
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
