#!/bin/sh
# Isolated Codex CLI probe for issue #31. No real Codex home or credentials used.
set -eu

codex_command=${CODEX_BIN:-codex}
codex_bin=$(command -v "$codex_command") || {
	printf 'codex executable not found: %s\n' "$codex_command" >&2
	exit 2
}
version=$("$codex_bin" --version)
if [ "$version" != 'codex-cli 0.157.0' ]; then
	printf 'expected codex-cli 0.157.0, found %s\n' "$version" >&2
	exit 2
fi

umask 077
probe=$(mktemp -d "${TMPDIR:-/tmp}/aix-codex-config-probe.XXXXXX")
trap 'rm -rf "$probe"' 0 HUP INT TERM
mkdir -p "$probe/empty-home/.codex" "$probe/selected" "$probe/file-home" "$probe/staged-home" "$probe/cwd"

fail() {
	printf 'FAIL: %s\n' "$1" >&2
	exit 1
}

run_codex() {
	codex_home=$1
	shift
	env -i PATH="$PATH" HOME="$probe/empty-home" CODEX_HOME="$codex_home" "$codex_bin" "$@"
}

# Call Codex with its actual default home or with an explicit alternate home.
run_default_codex() {
	env -i PATH="$PATH" HOME="$probe/empty-home" "$codex_bin" "$@"
}

write_feature_config() {
	printf 'features.multi_agent = %s\ncli_auth_credentials_store = "file"\n' "$2" >"$1/config.toml"
}

# CODEX_HOME chooses a different user config rather than merging both homes.
write_feature_config "$probe/empty-home/.codex" true
write_feature_config "$probe/selected" false
run_default_codex features list >"$probe/global.features" 2>"$probe/global.err" || fail 'default global home did not load'
run_codex "$probe/selected" features list >"$probe/selected.features" 2>"$probe/selected.err" || fail 'selected CODEX_HOME did not load'
grep -Eq '^multi_agent[[:space:]]+stable[[:space:]]+true$' "$probe/global.features" || fail 'global config marker was not effective'
grep -Eq '^multi_agent[[:space:]]+stable[[:space:]]+false$' "$probe/selected.features" || fail 'selected config marker was not effective'

# A trusted project config remains native Codex behavior under the selected home.
mkdir -p "$probe/project/.codex"
printf 'features.multi_agent = true\n' >"$probe/project/.codex/config.toml"
printf 'features.multi_agent = false\ncli_auth_credentials_store = "file"\n[projects."%s"]\ntrust_level = "trusted"\n' \
	"$probe/project" >"$probe/selected/config.toml"
(cd "$probe/project" && run_codex "$probe/selected" features list >"$probe/project.features" 2>"$probe/project.err") || fail 'trusted project config did not load under selected CODEX_HOME'
grep -Eq '^multi_agent[[:space:]]+stable[[:space:]]+true$' "$probe/project.features" || fail 'native project config was not applied'
(cd "$probe/cwd" && run_codex "$probe/selected" features list >"$probe/non-project.features" 2>"$probe/non-project.err") || fail 'selected user config did not load outside the project'
grep -Eq '^multi_agent[[:space:]]+stable[[:space:]]+false$' "$probe/non-project.features" || fail 'selected user config changed outside the project'

# Concurrent read-only starts with different homes keep their config values separate.
mkdir -p "$probe/concurrent-a" "$probe/concurrent-b"
write_feature_config "$probe/concurrent-a" true
write_feature_config "$probe/concurrent-b" false
run_codex "$probe/concurrent-a" features list >"$probe/concurrent-a.out" 2>"$probe/concurrent-a.err" &
first_pid=$!
run_codex "$probe/concurrent-b" features list >"$probe/concurrent-b.out" 2>"$probe/concurrent-b.err" &
second_pid=$!
wait "$first_pid" || fail 'first concurrent read-only start failed'
wait "$second_pid" || fail 'second concurrent read-only start failed'
grep -Eq '^multi_agent[[:space:]]+stable[[:space:]]+true$' "$probe/concurrent-a.out" || fail 'first concurrent home read the wrong config'
grep -Eq '^multi_agent[[:space:]]+stable[[:space:]]+false$' "$probe/concurrent-b.out" || fail 'second concurrent home read the wrong config'

# A setting present only in the normal home is not read under selected CODEX_HOME.
printf 'global_only = [\n' >"$probe/empty-home/.codex/config.toml"
run_codex "$probe/selected" features list >"$probe/replacement.out" 2>"$probe/replacement.err" || fail 'selected CODEX_HOME inherited invalid global config'
if run_default_codex features list >"$probe/global-invalid.out" 2>"$probe/global-invalid.err"; then
	fail 'invalid global config unexpectedly loaded'
fi
grep -q 'unclosed array' "$probe/global-invalid.err" || fail 'global config failure did not identify malformed TOML'

# --profile remains a layer over config.toml, not a replacement source.
printf 'invalid_selected = [\n' >"$probe/selected/config.toml"
printf 'model = "overlay-probe"\n' >"$probe/selected/overlay.config.toml"
if run_codex "$probe/selected" exec --profile overlay --strict-config --ephemeral probe >"$probe/overlay.out" 2>"$probe/overlay.err"; then
	fail '--profile unexpectedly bypassed the base user config'
fi
grep -F "$probe/selected/config.toml" "$probe/overlay.err" >/dev/null || fail '--profile failure did not come from base config.toml'

# A relative model_catalog_json path resolves from selected CODEX_HOME, not cwd.
printf 'model_catalog_json = "catalog.json"\ncli_auth_credentials_store = "file"\n' >"$probe/selected/config.toml"
cp "$probe/selected/config.toml" "$probe/selected.config.before"
printf 'not JSON\n' >"$probe/selected/catalog.json"
printf '{"models":[]}\n' >"$probe/cwd/catalog.json"
if (cd "$probe/cwd" && run_codex "$probe/selected" debug models >"$probe/catalog.out" 2>"$probe/catalog.err"); then
	fail 'malformed selected-home model catalog unexpectedly loaded'
fi
grep -F "$probe/selected/catalog.json" "$probe/catalog.err" >/dev/null || fail 'relative model catalog was not resolved from selected CODEX_HOME'
cmp -s "$probe/selected.config.before" "$probe/selected/config.toml" || fail 'Codex modified selected config.toml'

# A file-backed synthetic API key demonstrates the auth.json location only.
# It is fake, never used for a request, and kept in this temporary tree.
printf 'cli_auth_credentials_store = "file"\n' >"$probe/file-home/config.toml"
printf 'aix-probe-not-a-credential\n' | env -i PATH="$PATH" HOME="$probe/empty-home" CODEX_HOME="$probe/file-home" "$codex_bin" login --with-api-key >"$probe/login-save.out" 2>"$probe/login-save.err" || fail 'Codex did not write the isolated synthetic file credential'
test -f "$probe/file-home/auth.json" || fail 'file-mode auth.json was not under CODEX_HOME'
if run_codex "$probe/staged-home" login status >"$probe/no-auth.out" 2>&1; then
	fail 'an unrelated CODEX_HOME unexpectedly saw the file-backed credential'
fi
grep -q 'Not logged in' "$probe/no-auth.out" || fail 'empty CODEX_HOME did not report that no credential was found'
ln -s "$probe/file-home/auth.json" "$probe/staged-home/auth.json"
printf 'cli_auth_credentials_store = "file"\n' >"$probe/staged-home/config.toml"
run_codex "$probe/staged-home" login status >"$probe/shared-auth.out" 2>&1 || fail 'staged CODEX_HOME could not read linked auth.json'
grep -q 'Logged in using an API key' "$probe/shared-auth.out" || fail 'linked auth.json was not recognized'

printf 'PASS: CODEX_HOME replacement, --profile layering, and selected-home relative path\n'
printf 'PASS: native trusted project configuration remains active under selected CODEX_HOME\n'
printf 'PASS: concurrent read-only starts with different homes keep their config values separate\n'
printf 'PASS: file auth is home-scoped; a synthetic auth.json symlink is visible to Codex\n'
printf 'LIMIT: synthetic credential is not ChatGPT auth; sessions, history, keyring, and shared-state concurrency are not proven\n'
