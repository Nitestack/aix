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
