//! Agent-facing checker; shares the server's parser and verifier without depending on the runner.

use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

use analysis::tagged::{Context, MAX_BYTES};

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("usage: sr-check flow <job-dir>/out/flow.md | sr-check format")]
    Usage,
    #[error("cannot read the document or its verification context: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid verification context: {0}")]
    Context(#[from] serde_json::Error),
    #[error("{0}")]
    Rejected(String),
}

fn check() -> Result<(), Error> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "format" {
        print!(
            "{}",
            include_str!("../../../../docs/contracts/agent-output-format.md")
        );
        return Ok(());
    }
    if args.len() != 2 || args[0] != "flow" && args[0] != "catalog" {
        return Err(Error::Usage);
    }
    let file = Path::new(&args[1]);
    let job = file
        .parent()
        .and_then(Path::parent)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let context: Context =
        serde_json::from_reader(std::fs::File::open(job.join("flow-context.json"))?)?;
    if args[0] == "catalog" {
        println!("{}", serde_json::to_string_pretty(&context.catalog())?);
        return Ok(());
    }
    let mut text = String::new();
    std::fs::File::open(file)?
        .take(MAX_BYTES as u64 + 1)
        .read_to_string(&mut text)?;
    context
        .check(&text, &job.join("repo"))
        .map_err(|problems| {
            Error::Rejected(
                problems
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        })?;
    println!("ok");
    Ok(())
}

fn main() -> ExitCode {
    match check() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
