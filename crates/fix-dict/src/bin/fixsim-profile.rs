//! `fixsim-profile check <file.toml>`: checks a venue profile.
//!
//! It prints each error with its line and column, or the changes of the
//! profile with the full tree of repeating groups of each changed message.
//! The exit code is 0 for a valid profile, 1 for errors, and 2 for a wrong
//! command line or an unreadable file.

use fix_dict::{Profile, describe};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [command, path] = args.as_slice() else {
        eprintln!("usage: fixsim-profile check <file.toml>");
        return ExitCode::from(2);
    };
    if command != "check" {
        eprintln!("fixsim-profile: unknown command \"{command}\", use: check <file.toml>");
        return ExitCode::from(2);
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("fixsim-profile: cannot read {path}: {error}");
            return ExitCode::from(2);
        }
    };
    match Profile::load(&text) {
        Ok(venue) => {
            print!("{}", describe(&venue));
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for error in &errors {
                eprintln!("{path}:{error}");
            }
            eprintln!("{} error(s)", errors.len());
            ExitCode::FAILURE
        }
    }
}
