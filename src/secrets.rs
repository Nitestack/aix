#![allow(dead_code)]

use crate::error::AixError;
use serde::{Deserialize, Deserializer};
use std::fmt;
use std::path::PathBuf;
use zeroize::Zeroize;

/// Resolved secret value. Debug/Display never expose the contents.
/// The inner String is zeroed on drop.
pub struct SecretString(String);

impl Zeroize for SecretString {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl SecretString {
    pub fn new(s: String) -> Self {
        Self(s)
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[secret]")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[secret]")
    }
}

// ---------------------------------------------------------------------------
// SourceKind — private inner type shared by SecretSource and DynamicValue
// ---------------------------------------------------------------------------

pub(crate) enum SourceKind {
    Direct(String),
    Env(String),
    File(PathBuf),
    Command(String),
}

impl SourceKind {
    fn resolve_raw(&self) -> Result<String, AixError> {
        match self {
            SourceKind::Direct(s) => Ok(s.clone()),

            SourceKind::Env(name) => std::env::var(name)
                .map_err(|_| AixError::SecretMissingEnvVar { name: name.clone() }),

            SourceKind::File(raw_path) => {
                let expanded = shellexpand::tilde(&raw_path.to_string_lossy()).into_owned();
                let path = PathBuf::from(expanded);
                let content = std::fs::read_to_string(&path)
                    .map_err(|source| AixError::SecretFileRead { path, source })?;
                Ok(strip_one_trailing_newline(content))
            }

            SourceKind::Command(cmd) => {
                let output = run_command(cmd)?;
                Ok(strip_one_trailing_newline(output))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared deserialization helpers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(untagged)]
enum SourceKindDe {
    Direct(String),
    Structured(SourceKindFields),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceKindFields {
    env: Option<String>,
    file: Option<PathBuf>,
    command: Option<String>,
}

fn try_from_de<E: serde::de::Error>(de: SourceKindDe) -> Result<SourceKind, E> {
    match de {
        SourceKindDe::Direct(s) => Ok(SourceKind::Direct(s)),
        SourceKindDe::Structured(f) => match (f.env, f.file, f.command) {
            (Some(v), None, None) => Ok(SourceKind::Env(v)),
            (None, Some(p), None) => Ok(SourceKind::File(p)),
            (None, None, Some(c)) => Ok(SourceKind::Command(c)),
            (None, None, None) => Err(serde::de::Error::custom(
                "secret source table must specify one of: env, file, command",
            )),
            _ => Err(serde::de::Error::custom(
                "ambiguous secret source: specify exactly one of env / file / command",
            )),
        },
    }
}

// ---------------------------------------------------------------------------
// SecretSource — describes where to fetch a secret from
// ---------------------------------------------------------------------------

pub struct SecretSource(pub(crate) SourceKind);

/// Custom Debug hides Direct values to prevent accidental logging.
impl fmt::Debug for SecretSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            SourceKind::Direct(_) => f.write_str("SecretSource::Direct([redacted])"),
            SourceKind::Env(name) => write!(f, "SecretSource::Env({name:?})"),
            SourceKind::File(path) => write!(f, "SecretSource::File({path:?})"),
            SourceKind::Command(cmd) => write!(f, "SecretSource::Command({cmd:?})"),
        }
    }
}

impl SecretSource {
    /// Resolve the source to its secret value.
    ///
    /// Errors include the source type and identifier (env var name, file path,
    /// command string) but never the resolved value.
    pub fn resolve(&self) -> Result<SecretString, AixError> {
        self.0.resolve_raw().map(SecretString::new)
    }
}

impl<'de> Deserialize<'de> for SecretSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let de = SourceKindDe::deserialize(deserializer)?;
        try_from_de(de).map(SecretSource)
    }
}

// ---------------------------------------------------------------------------
// DynamicValue — dynamic but non-secret string source
// ---------------------------------------------------------------------------

pub struct DynamicValue(pub(crate) SourceKind);

impl fmt::Debug for DynamicValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            SourceKind::Direct(s) => write!(f, "DynamicValue::Direct({s:?})"),
            SourceKind::Env(name) => write!(f, "DynamicValue::Env({name:?})"),
            SourceKind::File(path) => write!(f, "DynamicValue::File({path:?})"),
            SourceKind::Command(cmd) => write!(f, "DynamicValue::Command({cmd:?})"),
        }
    }
}

impl DynamicValue {
    pub fn resolve(&self) -> Result<String, AixError> {
        self.0.resolve_raw()
    }
}

impl<'de> Deserialize<'de> for DynamicValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let de = SourceKindDe::deserialize(deserializer)?;
        try_from_de(de).map(DynamicValue)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Strip exactly one trailing newline (LF or CRLF). Does not trim any other
/// whitespace. This matches the convention of `pass`, `op`, and similar tools.
fn strip_one_trailing_newline(mut s: String) -> String {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    s
}

#[cfg(unix)]
fn run_command(cmd: &str) -> Result<String, AixError> {
    use std::process::Command;
    let output = Command::new("sh")
        .args(["-c", cmd])
        .output()
        .map_err(|source| AixError::SecretCommandSpawn {
            cmd: cmd.to_string(),
            source,
        })?;
    if !output.status.success() {
        return Err(AixError::SecretCommandFailed {
            cmd: cmd.to_string(),
            exit_code: output.status.code(),
        });
    }
    String::from_utf8(output.stdout).map_err(|_| AixError::SecretCommandEncoding {
        cmd: cmd.to_string(),
    })
}

#[cfg(windows)]
fn run_command(cmd: &str) -> Result<String, AixError> {
    use std::process::Command;
    let output = Command::new("cmd")
        .args(["/C", cmd])
        .output()
        .map_err(|source| AixError::SecretCommandSpawn {
            cmd: cmd.to_string(),
            source,
        })?;
    if !output.status.success() {
        return Err(AixError::SecretCommandFailed {
            cmd: cmd.to_string(),
            exit_code: output.status.code(),
        });
    }
    String::from_utf8(output.stdout).map_err(|_| AixError::SecretCommandEncoding {
        cmd: cmd.to_string(),
    })
}

#[cfg(not(any(unix, windows)))]
fn run_command(cmd: &str) -> Result<String, AixError> {
    Err(AixError::SecretCommandFailed {
        cmd: cmd.to_string(),
        exit_code: None,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- deserialization ---

    #[derive(Deserialize)]
    struct W {
        value: SecretSource,
    }

    fn from_toml(s: &str) -> Result<SecretSource, toml::de::Error> {
        toml::from_str::<W>(s).map(|w| w.value)
    }

    #[test]
    fn deser_direct() {
        let src = from_toml("value = \"sk-test\"").unwrap();
        assert!(matches!(src.0, SourceKind::Direct(ref s) if s == "sk-test"));
    }

    #[test]
    fn deser_env() {
        let src = from_toml("value = { env = \"MY_KEY\" }").unwrap();
        assert!(matches!(src.0, SourceKind::Env(ref s) if s == "MY_KEY"));
    }

    #[test]
    fn deser_file() {
        let src = from_toml("value = { file = \"/run/secrets/key\" }").unwrap();
        assert!(
            matches!(src.0, SourceKind::File(ref p) if p == std::path::Path::new("/run/secrets/key"))
        );
    }

    #[test]
    fn deser_command() {
        let src = from_toml("value = { command = \"op read op://Work/key\" }").unwrap();
        assert!(matches!(src.0, SourceKind::Command(ref s) if s == "op read op://Work/key"));
    }

    #[test]
    fn deser_ambiguous_multiple_keys() {
        assert!(from_toml("value = { env = \"X\", file = \"/y\" }").is_err());
    }

    #[test]
    fn deser_unknown_key_rejected() {
        assert!(from_toml("value = { keyring = \"X\" }").is_err());
    }

    #[test]
    fn deser_empty_table_rejected() {
        assert!(from_toml("value = {}").is_err());
    }

    // --- resolution: Direct ---

    #[test]
    fn resolve_direct() {
        let src = SecretSource(SourceKind::Direct("sk-direct".to_string()));
        assert_eq!(src.resolve().unwrap().expose_secret(), "sk-direct");
    }

    // --- resolution: Env ---

    #[test]
    fn resolve_env_set() {
        let var = "AIX_TEST_SEC_RESOLVE_ENV_SET_A1B2";
        std::env::set_var(var, "env-value-xyz");
        let result = SecretSource(SourceKind::Env(var.to_string())).resolve();
        std::env::remove_var(var);
        assert_eq!(result.unwrap().expose_secret(), "env-value-xyz");
    }

    #[test]
    fn resolve_env_missing() {
        let var = "AIX_TEST_SEC_RESOLVE_ENV_MISSING_X9Y8";
        std::env::remove_var(var);
        let err = SecretSource(SourceKind::Env(var.to_string())).resolve().unwrap_err();
        assert!(matches!(err, AixError::SecretMissingEnvVar { ref name } if name == var));
        let msg = err.to_string();
        assert!(msg.contains(var), "error must include var name");
    }

    // --- resolution: File ---

    #[test]
    fn resolve_file() {
        use assert_fs::prelude::*;
        let tmp = assert_fs::NamedTempFile::new("secret").unwrap();
        tmp.write_str("file-secret\n").unwrap();
        let s = SecretSource(SourceKind::File(tmp.path().to_path_buf()))
            .resolve()
            .unwrap();
        assert_eq!(s.expose_secret(), "file-secret");
    }

    #[test]
    fn resolve_file_strips_one_newline_only() {
        use assert_fs::prelude::*;
        let tmp = assert_fs::NamedTempFile::new("secret").unwrap();
        tmp.write_str("value\n\n").unwrap();
        let s = SecretSource(SourceKind::File(tmp.path().to_path_buf()))
            .resolve()
            .unwrap();
        assert_eq!(s.expose_secret(), "value\n");
    }

    #[test]
    fn resolve_file_no_trailing_newline() {
        use assert_fs::prelude::*;
        let tmp = assert_fs::NamedTempFile::new("secret").unwrap();
        tmp.write_str("no-newline").unwrap();
        let s = SecretSource(SourceKind::File(tmp.path().to_path_buf()))
            .resolve()
            .unwrap();
        assert_eq!(s.expose_secret(), "no-newline");
    }

    #[test]
    fn resolve_file_missing() {
        let err = SecretSource(SourceKind::File(PathBuf::from("/nonexistent/aix-secret-xyz")))
            .resolve()
            .unwrap_err();
        assert!(matches!(err, AixError::SecretFileRead { ref path, .. }
            if path.to_str().unwrap().contains("aix-secret-xyz")));
        let msg = err.to_string();
        assert!(msg.contains("aix-secret-xyz"), "error must include path");
    }

    // --- resolution: Command (Unix only) ---

    #[cfg(unix)]
    #[test]
    fn resolve_command_success() {
        let s = SecretSource(SourceKind::Command("printf 'cmd-secret'".to_string()))
            .resolve()
            .unwrap();
        assert_eq!(s.expose_secret(), "cmd-secret");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_command_strips_trailing_newline() {
        let s = SecretSource(SourceKind::Command("echo trailing".to_string()))
            .resolve()
            .unwrap();
        // echo appends \n; we strip exactly one
        assert_eq!(s.expose_secret(), "trailing");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_command_failure() {
        let err = SecretSource(SourceKind::Command("exit 42".to_string()))
            .resolve()
            .unwrap_err();
        assert!(
            matches!(
                err,
                AixError::SecretCommandFailed {
                    exit_code: Some(42),
                    ..
                }
            ),
            "unexpected error: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("exit 42"),
            "error must include command for diagnosis"
        );
        // no resolved api key value leaked
        assert!(!msg.contains("sk-"));
    }

    // --- no-leak guarantees ---

    #[test]
    fn secret_string_debug_hides_value() {
        let s = SecretString::new("sk-should-not-appear".to_string());
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("sk-should-not-appear"));
        assert_eq!(dbg, "[secret]");
    }

    #[test]
    fn secret_string_display_hides_value() {
        let s = SecretString::new("sk-should-not-appear".to_string());
        assert!(!s.to_string().contains("sk-should-not-appear"));
    }

    #[test]
    fn secret_source_debug_hides_direct_value() {
        let src = SecretSource(SourceKind::Direct("sk-direct-value".to_string()));
        let dbg = format!("{src:?}");
        assert!(!dbg.contains("sk-direct-value"));
        assert!(dbg.contains("redacted"));
    }

    #[test]
    fn secret_source_debug_shows_env_name() {
        let src = SecretSource(SourceKind::Env("MY_VAR".to_string()));
        let dbg = format!("{src:?}");
        assert!(dbg.contains("MY_VAR"));
    }

    // --- strip_one_trailing_newline ---

    #[test]
    fn strip_lf() {
        assert_eq!(strip_one_trailing_newline("hello\n".to_string()), "hello");
    }

    #[test]
    fn strip_crlf() {
        assert_eq!(strip_one_trailing_newline("hello\r\n".to_string()), "hello");
    }

    #[test]
    fn strip_no_newline() {
        assert_eq!(strip_one_trailing_newline("hello".to_string()), "hello");
    }

    #[test]
    fn strip_only_one_newline() {
        assert_eq!(
            strip_one_trailing_newline("hello\n\n".to_string()),
            "hello\n"
        );
    }

    // ---------------------------------------------------------------------------
    // DynamicValue tests
    // ---------------------------------------------------------------------------

    #[derive(Deserialize)]
    struct WDyn {
        value: DynamicValue,
    }

    fn from_toml_dyn(s: &str) -> Result<DynamicValue, toml::de::Error> {
        toml::from_str::<WDyn>(s).map(|w| w.value)
    }

    #[test]
    fn dynamic_value_deser_direct() {
        let dv = from_toml_dyn("value = \"Work\"").unwrap();
        assert!(matches!(dv.0, SourceKind::Direct(ref s) if s == "Work"));
    }

    #[test]
    fn dynamic_value_deser_env() {
        let dv = from_toml_dyn("value = { env = \"MY_LABEL\" }").unwrap();
        assert!(matches!(dv.0, SourceKind::Env(ref s) if s == "MY_LABEL"));
    }

    #[test]
    fn dynamic_value_deser_file() {
        let dv = from_toml_dyn("value = { file = \"/run/labels/work\" }").unwrap();
        assert!(
            matches!(dv.0, SourceKind::File(ref p) if p == std::path::Path::new("/run/labels/work"))
        );
    }

    #[test]
    fn dynamic_value_deser_command() {
        let dv = from_toml_dyn("value = { command = \"pass show label\" }").unwrap();
        assert!(matches!(dv.0, SourceKind::Command(ref s) if s == "pass show label"));
    }

    #[test]
    fn dynamic_value_deser_empty_table_rejected() {
        assert!(from_toml_dyn("value = {}").is_err());
    }

    #[test]
    fn dynamic_value_deser_ambiguous_rejected() {
        assert!(from_toml_dyn("value = { env = \"X\", file = \"/y\" }").is_err());
    }

    #[test]
    fn dynamic_value_resolve_direct() {
        let dv = from_toml_dyn("value = \"Work account\"").unwrap();
        assert_eq!(dv.resolve().unwrap(), "Work account");
    }

    #[test]
    fn dynamic_value_resolve_env_set() {
        let var = "AIX_TEST_DYN_RESOLVE_ENV_V1Q2";
        std::env::set_var(var, "env-label");
        let dv = from_toml_dyn(&format!("value = {{ env = \"{var}\" }}")).unwrap();
        let result = dv.resolve();
        std::env::remove_var(var);
        assert_eq!(result.unwrap(), "env-label");
    }

    #[test]
    fn dynamic_value_resolve_env_missing() {
        let var = "AIX_TEST_DYN_RESOLVE_ENV_MISSING_Z9W8";
        std::env::remove_var(var);
        let dv = from_toml_dyn(&format!("value = {{ env = \"{var}\" }}")).unwrap();
        let err = dv.resolve().unwrap_err();
        assert!(matches!(err, AixError::SecretMissingEnvVar { ref name } if name == var));
    }

    #[test]
    fn dynamic_value_resolve_file() {
        use assert_fs::prelude::*;
        let tmp = assert_fs::NamedTempFile::new("label").unwrap();
        tmp.write_str("Work account\n").unwrap();
        let dv = DynamicValue(SourceKind::File(tmp.path().to_path_buf()));
        assert_eq!(dv.resolve().unwrap(), "Work account");
    }

    #[test]
    fn dynamic_value_debug_shows_direct_value() {
        let dv = from_toml_dyn("value = \"Work\"").unwrap();
        let dbg = format!("{dv:?}");
        assert!(dbg.contains("Work"), "direct label value must be visible in debug: {dbg}");
    }

    #[test]
    fn dynamic_value_debug_shows_env_name() {
        let dv = from_toml_dyn("value = { env = \"MY_LABEL\" }").unwrap();
        let dbg = format!("{dv:?}");
        assert!(dbg.contains("MY_LABEL"));
    }
}
