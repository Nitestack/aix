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
    let envelope = JsonEnvelope {
        schema_version: 1,
        command,
        data,
    };
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, &envelope)?;
    stdout.write_all(b"\n")?;
    Ok(())
}
