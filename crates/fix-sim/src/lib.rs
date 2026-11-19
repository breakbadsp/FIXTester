//! A deterministic simulator for two FIX sessions, in the style of the VOPR
//! of TigerBeetle. One seed decides everything: the FIX version, the
//! heartbeat intervals, the network delays and chunking, the faults, and the
//! application traffic. A failing seed reproduces the same run every time.
//!
//! An initiator [`fix_session::Session`] and an acceptor session run on a
//! simulated clock. They talk over two simulated TCP pipes. The host loop of
//! the simulator follows the contract in the `fix-session` documentation.

mod host;
mod pipe;
mod plan;
mod rng;
mod world;

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

pub use host::Stats;
pub use rng::Rng;
pub use world::{Options, Report, START, World};

/// A run that broke an invariant, or panicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimFailure {
    pub seed: u64,
    /// The simulated time of the failure, in ms after the start.
    pub at_ms: u64,
    pub message: String,
}

impl fmt::Display for SimFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "seed {} failed at +{} ms: {}",
            self.seed, self.at_ms, self.message
        )
    }
}

impl std::error::Error for SimFailure {}

/// Runs the simulation for `seed`. A panic in the code under test is a
/// failure of the seed, not a crash of the caller.
pub fn run_seed(seed: u64) -> Result<Report, SimFailure> {
    run_seed_with(seed, Options::default())
}

/// Like [`run_seed`], with settings that the seed does not decide.
pub fn run_seed_with(seed: u64, options: Options) -> Result<Report, SimFailure> {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let mut world = World::new(seed, options);
        world
            .run()
            .map_err(|message| (world.now().0.saturating_sub(START.0), message))
    }));
    match outcome {
        Ok(Ok(report)) => Ok(report),
        Ok(Err((at_ms, message))) => Err(SimFailure {
            seed,
            at_ms,
            message,
        }),
        Err(payload) => {
            let text = payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_owned());
            Err(SimFailure {
                seed,
                at_ms: 0,
                message: format!("panic: {text}"),
            })
        }
    }
}
