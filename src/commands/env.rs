use crate::cli::EnvFormat;
use crate::config;
use crate::error::AixError;
use color_eyre::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn run(
    positional_profile: Option<String>,
    config_path: Option<PathBuf>,
    format: EnvFormat,
) -> Result<()> {
    let path = config::find_config_path(config_path.as_deref())?.ok_or(AixError::NoConfigFile)?;
    let cfg = config::load(&path)?;
    config::validate(&cfg)?;
    config::load_env_files(&cfg)?;

    let profile_name = resolve_profile(positional_profile, &cfg)?;

    let profile = cfg
        .profiles
        .get(&profile_name)
        .ok_or_else(|| AixError::ProfileNotFound {
            name: profile_name.clone(),
            available_hint: config::format_available_profiles(&cfg),
        })?;

    let api_key = profile.api_key.resolve()?;
    let base_url = cfg.endpoint.base_url.resolve()?;

    let vars = collect_vars(
        &profile_name,
        api_key.expose_secret(),
        base_url.expose_secret(),
        &cfg.endpoint.api_format,
    );

    print!("{}", format_vars(&vars, &format));
    Ok(())
}

pub(crate) fn collect_vars(
    profile_name: &str,
    api_key: &str,
    base_url: &str,
    api_format: &config::ApiFormat,
) -> Vec<(&'static str, String)> {
    use config::ApiFormat;
    let mut vars = vec![
        ("AIX_PROFILE", profile_name.to_string()),
        ("AIX_API_KEY", api_key.to_string()),
        ("AIX_BASE_URL", base_url.to_string()),
    ];
    let emit_anthropic = matches!(api_format, ApiFormat::Anthropic | ApiFormat::Both);
    let emit_openai = matches!(api_format, ApiFormat::OpenAi | ApiFormat::Both);
    if emit_anthropic {
        vars.push(("ANTHROPIC_API_KEY", api_key.to_string()));
        vars.push(("ANTHROPIC_BASE_URL", base_url.to_string()));
    }
    if emit_openai {
        vars.push(("OPENAI_API_KEY", api_key.to_string()));
        vars.push(("OPENAI_BASE_URL", base_url.to_string()));
    }
    vars
}

fn format_vars(vars: &[(&'static str, String)], format: &EnvFormat) -> String {
    match format {
        EnvFormat::Sh => format_sh(vars),
        EnvFormat::Json => format_json(vars),
        EnvFormat::Nu => format_nu(vars),
        EnvFormat::Fish => format_fish(vars),
        EnvFormat::Powershell => format_powershell(vars),
        EnvFormat::Cmd => format_cmd(vars),
    }
}

fn sh_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn format_sh(vars: &[(&'static str, String)]) -> String {
    let mut out = vars
        .iter()
        .map(|(k, v)| format!("export {}={}", k, sh_escape(v)))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

fn nu_escape(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}

fn format_nu(vars: &[(&'static str, String)]) -> String {
    let mut out = vars
        .iter()
        .map(|(k, v)| format!("$env.{} = {}", k, nu_escape(v)))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

fn fish_escape(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn format_fish(vars: &[(&'static str, String)]) -> String {
    let mut out = vars
        .iter()
        .map(|(k, v)| format!("set -x {} {}", k, fish_escape(v)))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

fn ps_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn format_powershell(vars: &[(&'static str, String)]) -> String {
    let mut out = vars
        .iter()
        .map(|(k, v)| format!("$env:{} = {}", k, ps_escape(v)))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

// Escape a value for use in:  set "VAR=<escaped>"
//
// Within the cmd.exe `set "VAR=..."` quoted form:
//   %  →  %%   (prevents batch-file variable expansion before the SET runs)
//   "  →  ""   (closest safe encoding; see LIMITATION below)
//
// Characters that are literal and need no escaping within `set "..."`:
//   ^ & | < > ( ) spaces ' $ \ and all Unicode code points.
//
// LIMITATION — double quote in values:
//   The `set "VAR=..."` form has no reliable escape for `"`.  The `""` encoding
//   used here works on most Windows 10/11 cmd.exe versions but is not guaranteed
//   on all NT versions.  In practice, API keys and URLs never contain `"`, so this
//   is rarely relevant.  Windows users whose values do contain `"` should use
//   `--format powershell` instead.
//
// LIMITATION — delayed expansion (`!`):
//   When `setlocal enabledelayedexpansion` is active, `!VAR!` is expanded before
//   the SET command runs and `!` cannot be safely escaped within `set "..."`.
//   The generated `.cmd` file must not be called inside a
//   `setlocal enabledelayedexpansion` block for values that contain `!`.
fn cmd_escape(s: &str) -> String {
    s.replace('%', "%%").replace('"', "\"\"")
}

fn format_cmd(vars: &[(&'static str, String)]) -> String {
    let mut out = vars
        .iter()
        .map(|(k, v)| format!("set \"{}={}\"", k, cmd_escape(v)))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

fn format_json(vars: &[(&'static str, String)]) -> String {
    let map: BTreeMap<&str, &str> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut out =
        serde_json::to_string_pretty(&map).expect("BTreeMap<&str,&str> is always serializable");
    out.push('\n');
    out
}

pub(crate) fn resolve_profile(
    positional: Option<String>,
    cfg: &config::Config,
) -> Result<String, AixError> {
    if let Some(p) = positional {
        return Ok(p);
    }
    if let Some(p) = cfg.default_profile.clone() {
        return Ok(p);
    }
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        select_profile_interactively(cfg)
    } else {
        Err(AixError::NoInteractiveTerminal)
    }
}

fn select_profile_interactively(cfg: &config::Config) -> Result<String, AixError> {
    let profiles = config::sorted_profiles(cfg);
    let options: Vec<String> = profiles
        .iter()
        .map(|(_, label)| label.to_string())
        .collect();

    let selected = inquire::Select::new("Select a profile:", options)
        .prompt()
        .map_err(|_| AixError::SelectionCancelled)?;

    // validate() has already rejected duplicate labels, so this find is unambiguous.
    profiles
        .into_iter()
        .find(|(_, label)| *label == selected.as_str())
        .map(|(name, _)| name.to_string())
        .ok_or_else(|| AixError::ProfileNotFound {
            name: selected,
            available_hint: config::format_available_profiles(cfg),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ApiFormat;

    // --- collect_vars with ApiFormat ---

    #[test]
    fn collect_vars_anthropic_produces_5_vars() {
        let vars = collect_vars(
            "swtb",
            "sk-key",
            "https://example.com",
            &ApiFormat::Anthropic,
        );
        assert_eq!(vars.len(), 5);
        assert_eq!(vars[0], ("AIX_PROFILE", "swtb".to_string()));
        assert_eq!(vars[1], ("AIX_API_KEY", "sk-key".to_string()));
        assert_eq!(vars[2], ("AIX_BASE_URL", "https://example.com".to_string()));
        assert_eq!(vars[3], ("ANTHROPIC_API_KEY", "sk-key".to_string()));
        assert_eq!(
            vars[4],
            ("ANTHROPIC_BASE_URL", "https://example.com".to_string())
        );
    }

    #[test]
    fn collect_vars_anthropic_has_no_openai_vars() {
        let vars = collect_vars("p", "k", "u", &ApiFormat::Anthropic);
        let names: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
        assert!(!names.contains(&"OPENAI_API_KEY"));
        assert!(!names.contains(&"OPENAI_BASE_URL"));
    }

    #[test]
    fn collect_vars_openai_produces_5_vars() {
        let vars = collect_vars("swtb", "sk-key", "https://example.com", &ApiFormat::OpenAi);
        assert_eq!(vars.len(), 5);
        assert_eq!(vars[3], ("OPENAI_API_KEY", "sk-key".to_string()));
        assert_eq!(
            vars[4],
            ("OPENAI_BASE_URL", "https://example.com".to_string())
        );
    }

    #[test]
    fn collect_vars_openai_has_no_anthropic_vars() {
        let vars = collect_vars("p", "k", "u", &ApiFormat::OpenAi);
        let names: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
        assert!(!names.contains(&"ANTHROPIC_API_KEY"));
        assert!(!names.contains(&"ANTHROPIC_BASE_URL"));
    }

    #[test]
    fn collect_vars_both_produces_7_vars() {
        let vars = collect_vars("swtb", "sk-key", "https://example.com", &ApiFormat::Both);
        assert_eq!(vars.len(), 7);
        assert_eq!(vars[3], ("ANTHROPIC_API_KEY", "sk-key".to_string()));
        assert_eq!(
            vars[4],
            ("ANTHROPIC_BASE_URL", "https://example.com".to_string())
        );
        assert_eq!(vars[5], ("OPENAI_API_KEY", "sk-key".to_string()));
        assert_eq!(
            vars[6],
            ("OPENAI_BASE_URL", "https://example.com".to_string())
        );
    }

    #[test]
    fn collect_vars_always_emits_aix_base_vars() {
        for fmt in [ApiFormat::Anthropic, ApiFormat::OpenAi, ApiFormat::Both] {
            let vars = collect_vars("p", "k", "u", &fmt);
            let names: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
            assert!(names.contains(&"AIX_PROFILE"), "missing for {fmt:?}");
            assert!(names.contains(&"AIX_API_KEY"), "missing for {fmt:?}");
            assert!(names.contains(&"AIX_BASE_URL"), "missing for {fmt:?}");
        }
    }

    // --- sh ---

    #[test]
    fn sh_escape_plain_value() {
        assert_eq!(sh_escape("hello"), "'hello'");
    }

    #[test]
    fn sh_escape_single_quote_in_value() {
        assert_eq!(sh_escape("it's"), r"'it'\''s'");
    }

    #[test]
    fn sh_escape_url_with_query_string() {
        assert_eq!(
            sh_escape("https://x.com/v1?a=b&c=d"),
            "'https://x.com/v1?a=b&c=d'"
        );
    }

    #[test]
    fn format_sh_emits_export_lines() {
        let vars = vec![("FOO", "bar".to_string()), ("BAZ", "qux".to_string())];
        assert_eq!(format_sh(&vars), "export FOO='bar'\nexport BAZ='qux'\n");
    }

    #[test]
    fn format_sh_escapes_single_quotes() {
        let vars = vec![("KEY", "it's a 'test'".to_string())];
        let out = format_sh(&vars);
        assert_eq!(out, "export KEY='it'\\''s a '\\''test'\\'''\n");
    }

    // --- json ---

    #[test]
    fn format_json_produces_valid_json_object() {
        let vars = vec![("FOO", "bar".to_string()), ("BAZ", "42".to_string())];
        let out = format_json(&vars);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["FOO"], "bar");
        assert_eq!(parsed["BAZ"], "42");
    }

    #[test]
    fn format_json_handles_double_quotes_in_values() {
        let vars = vec![("KEY", r#"say "hi""#.to_string())];
        let out = format_json(&vars);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["KEY"], r#"say "hi""#);
    }

    #[test]
    fn format_json_handles_backslash_in_values() {
        let vars = vec![("KEY", r"a\b".to_string())];
        let out = format_json(&vars);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["KEY"].as_str().unwrap(), r"a\b");
    }

    // --- nu ---

    #[test]
    fn nu_escape_plain_value() {
        assert_eq!(nu_escape("hello"), "\"hello\"");
    }

    #[test]
    fn nu_escape_double_quote_in_value() {
        assert_eq!(nu_escape(r#"say "hi""#), r#""say \"hi\"""#);
    }

    #[test]
    fn nu_escape_backslash_in_value() {
        assert_eq!(nu_escape(r"a\b"), r#""a\\b""#);
    }

    #[test]
    fn format_nu_emits_env_assignment() {
        let vars = vec![("FOO", "bar".to_string())];
        assert_eq!(format_nu(&vars), "$env.FOO = \"bar\"\n");
    }

    // --- fish ---

    #[test]
    fn fish_escape_plain_value() {
        assert_eq!(fish_escape("hello"), "'hello'");
    }

    #[test]
    fn fish_escape_single_quote_in_value() {
        assert_eq!(fish_escape("it's"), r"'it\'s'");
    }

    #[test]
    fn fish_escape_backslash_in_value() {
        assert_eq!(fish_escape(r"a\b"), r"'a\\b'");
    }

    #[test]
    fn format_fish_emits_set_x() {
        let vars = vec![("FOO", "bar".to_string())];
        assert_eq!(format_fish(&vars), "set -x FOO 'bar'\n");
    }

    // --- powershell ---

    #[test]
    fn ps_escape_plain_value() {
        assert_eq!(ps_escape("hello"), "'hello'");
    }

    #[test]
    fn ps_escape_single_quote_in_value() {
        assert_eq!(ps_escape("it's"), "'it''s'");
    }

    #[test]
    fn format_powershell_emits_env_colon_assignment() {
        let vars = vec![("FOO", "bar".to_string())];
        assert_eq!(format_powershell(&vars), "$env:FOO = 'bar'\n");
    }

    // --- cmd ---

    #[test]
    fn cmd_escape_plain_value() {
        assert_eq!(cmd_escape("hello"), "hello");
    }

    #[test]
    fn cmd_escape_spaces() {
        // spaces are safe within set "VAR=..."
        assert_eq!(cmd_escape("with spaces"), "with spaces");
    }

    #[test]
    fn cmd_escape_single_quote() {
        // single quote is safe within set "VAR=..."
        assert_eq!(cmd_escape("it's"), "it's");
    }

    #[test]
    fn cmd_escape_double_quote() {
        // " is doubled; see limitation note on cmd_escape
        assert_eq!(cmd_escape(r#"say "hi""#), r#"say ""hi"""#);
    }

    #[test]
    fn cmd_escape_dollar_sign() {
        assert_eq!(cmd_escape("$VAR"), "$VAR");
    }

    #[test]
    fn cmd_escape_backslash() {
        assert_eq!(cmd_escape(r"C:\path"), r"C:\path");
    }

    #[test]
    fn cmd_escape_ampersand() {
        // & is literal within set "VAR=..."
        assert_eq!(cmd_escape("a&b"), "a&b");
    }

    #[test]
    fn cmd_escape_pipe() {
        // | is literal within set "VAR=..."
        assert_eq!(cmd_escape("a|b"), "a|b");
    }

    #[test]
    fn cmd_escape_less_than() {
        assert_eq!(cmd_escape("a<b"), "a<b");
    }

    #[test]
    fn cmd_escape_greater_than() {
        assert_eq!(cmd_escape("a>b"), "a>b");
    }

    #[test]
    fn cmd_escape_caret() {
        // ^ is literal within set "VAR=..." (not an escape char in quoted mode)
        assert_eq!(cmd_escape("a^b"), "a^b");
    }

    #[test]
    fn cmd_escape_percent() {
        assert_eq!(cmd_escape("100%"), "100%%");
    }

    #[test]
    fn cmd_escape_multiple_percent() {
        assert_eq!(cmd_escape("a%b%c"), "a%%b%%c");
    }

    #[test]
    fn cmd_escape_parentheses() {
        assert_eq!(cmd_escape("(foo)"), "(foo)");
    }

    #[test]
    fn cmd_escape_unicode() {
        assert_eq!(cmd_escape("ä"), "ä");
    }

    #[test]
    fn format_cmd_emits_set_quoted_lines() {
        let vars = vec![("FOO", "bar".to_string()), ("BAZ", "qux".to_string())];
        assert_eq!(format_cmd(&vars), "set \"FOO=bar\"\nset \"BAZ=qux\"\n");
    }

    #[test]
    fn format_cmd_escapes_percent() {
        let vars = vec![("KEY", "100%".to_string())];
        let out = format_cmd(&vars);
        assert_eq!(out, "set \"KEY=100%%\"\n");
    }

    #[test]
    fn format_cmd_emits_trailing_newline() {
        let vars = vec![("FOO", "bar".to_string())];
        let out = format_cmd(&vars);
        assert!(out.ends_with('\n'));
    }
}
