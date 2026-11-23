//! Runs the simulator over many seeds. A failure names the seed. To replay
//! one seed: `cargo run -p fix-sim --bin fix_sim -- <seed> 1`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use fix_sim::{Options, Report, run_seed, run_seed_with};

const SEEDS: u64 = 600;

fn failures(options: Options) -> Vec<String> {
    (0..SEEDS)
        .filter_map(|seed| run_seed_with(seed, options).err())
        .map(|failure| failure.to_string())
        .collect()
}

/// A run that never used a fault would pass for the wrong reason.
#[test]
fn sessions_recover_across_seeds_that_exercise_every_fault() {
    let mut total = Report::default();
    let mut failures = Vec::new();
    for seed in 0..SEEDS {
        let report = match run_seed(seed) {
            Ok(report) => report,
            Err(failure) => {
                failures.push(failure.to_string());
                continue;
            }
        };
        total.cuts += report.cuts;
        total.stalls += report.stalls;
        total.clock_steps += report.clock_steps;
        total.logouts += report.logouts;
        total.duplicates += report.duplicates;
        total.refused_sends += report.refused_sends;
        for (sum, stats) in total.stats.iter_mut().zip(&report.stats) {
            sum.delivered += stats.delivered;
            sum.gaps_detected += stats.gaps_detected;
            sum.messages_dropped += stats.messages_dropped;
            sum.logons += stats.logons;
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {SEEDS} seeds failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(total.cuts > 0 && total.stalls > 0 && total.clock_steps > 0 && total.logouts > 0);
    assert!(total.duplicates > 0, "no duplicate was injected");
    assert!(
        total.refused_sends > 0,
        "no order was tried while not active"
    );
    for (side, stats) in total.stats.iter().enumerate() {
        assert!(stats.delivered > 0, "side {side} delivered nothing");
        assert!(stats.gaps_detected > 0, "side {side} saw no gap");
        assert!(stats.messages_dropped > 0, "side {side} lost no message");
        assert!(
            stats.logons > SEEDS,
            "side {side} did not log on again after a disconnect"
        );
    }
}

#[test]
fn the_same_seed_gives_the_same_run() {
    for seed in [1, 17, 404] {
        let first = run_seed(seed).unwrap();
        let second = run_seed(seed).unwrap();
        assert_eq!(first.steps, second.steps, "seed {seed}");
        assert_eq!(first.stats, second.stats, "seed {seed}");
    }
}

#[test]
fn sessions_recover_when_a_resend_request_or_gap_fill_is_lost() {
    let failures = failures(Options {
        drop_recovery: true,
    });
    assert!(
        failures.is_empty(),
        "{} of {SEEDS} seeds failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
