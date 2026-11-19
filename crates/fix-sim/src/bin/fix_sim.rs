//! `fix_sim <first_seed> <count>`: runs the simulation for `count` seeds from
//! `first_seed`. Prints each failing seed and a summary. Exits with 1 when a
//! seed fails and with 2 on a usage error.

use std::process::ExitCode;

use fix_sim::{Options, Report, run_seed_with};

fn parse_args() -> Result<(u64, u64), String> {
    let usage = "usage: fix_sim <first_seed> <count>";
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [first, count] = args.as_slice() else {
        return Err(usage.to_owned());
    };
    let number = |text: &str| {
        text.parse::<u64>()
            .map_err(|error| format!("{usage}: {error}"))
    };
    Ok((number(first)?, number(count)?))
}

fn main() -> ExitCode {
    let (first, count) = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let mut failures = 0_u64;
    let mut total = Report::default();
    for seed in first..first.saturating_add(count) {
        // Odd seeds also lose ResendRequests and gap fills.
        let options = Options {
            drop_recovery: seed % 2 == 1,
        };
        match run_seed_with(seed, options) {
            Ok(report) => {
                total.cuts += report.cuts;
                total.stalls += report.stalls;
                total.clock_steps += report.clock_steps;
                total.logouts += report.logouts;
                total.steps += report.steps;
                total.simulated_ms += report.simulated_ms;
                total.stats[0].delivered += report.stats[0].delivered;
                total.stats[1].delivered += report.stats[1].delivered;
                total.stats[0].gaps_detected += report.stats[0].gaps_detected;
                total.stats[1].gaps_detected += report.stats[1].gaps_detected;
                total.stats[0].logons += report.stats[0].logons;
            }
            Err(failure) => {
                failures += 1;
                println!("{failure}");
            }
        }
    }
    println!(
        "seeds {first}..{}: {failures} failed; {} simulated s, {} steps, {} cuts, {} stalls, \
         {} clock steps, {} logouts, {} logons, {} gaps, {} messages delivered",
        first.saturating_add(count),
        total.simulated_ms / 1000,
        total.steps,
        total.cuts,
        total.stalls,
        total.clock_steps,
        total.logouts,
        total.stats[0].logons,
        total.stats[0].gaps_detected + total.stats[1].gaps_detected,
        total.stats[0].delivered + total.stats[1].delivered,
    );
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
