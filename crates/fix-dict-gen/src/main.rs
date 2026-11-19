//! Generates `crates/fix-dict/src/generated/{fix42,fix43,fix44}.rs` from the
//! FIX Unified Repository XML. Run it with `tools/regen-fix-dict.sh`.
//!
//! ```text
//! fix-dict-gen --input FixRepository.xml --out-dir DIR --source-sha256 HEX
//!              [--source-name NAME]
//! ```

#![forbid(unsafe_code)]

mod emit;
mod error;
mod model;
mod resolve;
mod xml;

use error::GenError;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

/// `(repository version, output file name)`.
const VERSIONS: [(&str, &str); 3] = [
    ("FIX.4.2", "fix42.rs"),
    ("FIX.4.3", "fix43.rs"),
    ("FIX.4.4", "fix44.rs"),
];

struct Args {
    input: PathBuf,
    out_dir: PathBuf,
    source_sha256: String,
    source_name: String,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Args, GenError> {
    let mut input = None;
    let mut out_dir = None;
    let mut source_sha256 = None;
    let mut source_name = "FIX Unified Repository 2010 Edition (FIX Trading Community), \
         repository.zip, entry Unified/FixRepository.xml"
        .to_owned();
    let mut args = args;
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| GenError::Usage(format!("{flag} needs a value")))?;
        match flag.as_str() {
            "--input" => input = Some(PathBuf::from(value)),
            "--out-dir" => out_dir = Some(PathBuf::from(value)),
            "--source-sha256" => source_sha256 = Some(value),
            "--source-name" => source_name = value,
            other => return Err(GenError::Usage(format!("unknown flag {other}"))),
        }
    }
    let missing = |name: &str| GenError::Usage(format!("{name} is required"));
    let source_sha256 = source_sha256.ok_or_else(|| missing("--source-sha256"))?;
    if source_sha256.len() != 64 || !source_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(GenError::Usage(
            "--source-sha256 is not 64 hex digits".to_owned(),
        ));
    }
    Ok(Args {
        input: input.ok_or_else(|| missing("--input"))?,
        out_dir: out_dir.ok_or_else(|| missing("--out-dir"))?,
        source_sha256,
        source_name,
    })
}

/// Formats `source` with `rustfmt`. It runs in `dir`, so it finds the
/// `rustfmt.toml` of the repository.
fn rustfmt(source: &str, dir: &Path) -> Result<String, GenError> {
    let io_error = |source| GenError::Io {
        context: "rustfmt".to_owned(),
        source,
    };
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout"])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io_error)?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| GenError::Rustfmt("no stdin".to_owned()))?;
    let input = source.to_owned();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child.wait_with_output().map_err(io_error)?;
    let _ = writer.join();
    if !output.status.success() {
        return Err(GenError::Rustfmt(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| GenError::Rustfmt("output is not UTF-8".to_owned()))
}

fn run(args: &Args) -> Result<(), GenError> {
    let text = std::fs::read_to_string(&args.input).map_err(|source| GenError::Io {
        context: format!("read {}", args.input.display()),
        source,
    })?;
    let names: Vec<&str> = VERSIONS.iter().map(|(version, _)| *version).collect();
    let parsed = xml::parse_versions(&text, &names)?;
    let notice = emit::Notice {
        source_name: &args.source_name,
        source_sha256: &args.source_sha256,
        copyright: &parsed.copyright,
    };
    for (version, file) in VERSIONS {
        let node = parsed
            .versions
            .iter()
            .find(|node| node.attr("version") == Some(version))
            .ok_or_else(|| GenError::data(format!("the repository has no {version}")))?;
        let dict = resolve::resolve(&model::Repository::from_node(node)?)?;
        let source = emit::emit(&dict, &notice)?;
        let formatted = rustfmt(&source, &args.out_dir)?;
        let path = args.out_dir.join(file);
        std::fs::write(&path, formatted).map_err(|source| GenError::Io {
            context: format!("write {}", path.display()),
            source,
        })?;
        eprintln!(
            "{version}: {} fields, {} messages, {} groups -> {}",
            dict.fields.len(),
            dict.messages.len(),
            dict.groups.len(),
            path.display()
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let result = parse_args(std::env::args().skip(1)).and_then(|args| run(&args));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fix-dict-gen: {error}");
            ExitCode::FAILURE
        }
    }
}
