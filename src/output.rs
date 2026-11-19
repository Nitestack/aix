use color_eyre::Result;
use serde::Serialize;
use std::io::Write;

#[derive(Serialize)]
struct JsonEnvelope<'a, T> {
    schema_version: u8,
    command: &'a str,
    data: T,
}

pub fn print_json<T: Serialize>(command: &str, data: T) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    print_json_to(command, data, &mut stdout)
}

pub(crate) fn print_json_to<T: Serialize, W: Write>(
    command: &str,
    data: T,
    output: &mut W,
) -> Result<()> {
    let envelope = JsonEnvelope {
        schema_version: 1,
        command,
        data,
    };
    serde_json::to_writer_pretty(&mut *output, &envelope)?;
    output.write_all(b"\n")?;
    Ok(())
}

#[derive(Serialize)]
struct JsonErrorEnvelope<'a> {
    schema_version: u8,
    command: &'a str,
    error: JsonError,
}

#[derive(Serialize)]
struct JsonError {
    code: &'static str,
    category: &'static str,
    exit_code: i32,
    message: String,
}

/// Write a failed command's error as one JSON document to stderr.
pub fn print_json_error(command: &str, error: &color_eyre::Report) {
    let mut stderr = std::io::stderr().lock();
    let _ = print_json_error_to(command, error, &mut stderr);
}

pub(crate) fn print_json_error_to<W: Write>(
    command: &str,
    error: &color_eyre::Report,
    output: &mut W,
) -> Result<()> {
    let exit_code = crate::error::exit_code(error);
    write_json_error(
        command,
        crate::error::error_code(error),
        exit_code,
        format!("{error:#}"),
        output,
    )
}

/// Write a command-line parsing error as one JSON document to stderr.
pub fn print_json_usage_error(exit_code: i32, message: String) {
    let mut stderr = std::io::stderr().lock();
    let _ = write_json_error("aix", "invalid_arguments", exit_code, message, &mut stderr);
}

fn write_json_error<W: Write>(
    command: &str,
    code: &'static str,
    exit_code: i32,
    message: String,
    output: &mut W,
) -> Result<()> {
    let envelope = JsonErrorEnvelope {
        schema_version: 1,
        command,
        error: JsonError {
            code,
            category: crate::error::exit_category(exit_code),
            exit_code,
            message,
        },
    };
    serde_json::to_writer(&mut *output, &envelope)?;
    output.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_error_envelope_has_a_stable_code_and_category() {
        let error: color_eyre::Report = crate::error::AixError::ProfileNotFound {
            name: "missing".to_string(),
            available_hint: "  work".to_string(),
        }
        .into();
        let mut out = Vec::new();
        print_json_error_to("status", &error, &mut out).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["command"], "status");
        assert_eq!(value["error"]["code"], "profile_not_found");
        assert_eq!(value["error"]["category"], "config");
        assert_eq!(value["error"]["exit_code"], 2);
        assert!(value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("profile \"missing\" is not defined"));
    }
}
