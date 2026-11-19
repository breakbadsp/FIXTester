//! What one seed decides: the FIX version, the heartbeat intervals, the
//! length of the fault phase, and how often each kind of fault happens.

use crate::rng::Rng;

const VERSIONS: [&[u8]; 3] = [b"FIX.4.2", b"FIX.4.3", b"FIX.4.4"];
const HEARTBEATS_MS: [u64; 4] = [1_000, 2_000, 5_000, 10_000];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Both sides lose the connection. Bytes in flight are lost.
    Cut,
    /// The next message of one side never reaches the wire.
    Drop,
    /// One direction delivers nothing for a while, then all at once.
    Stall,
    /// The clock of one side jumps.
    ClockStep,
    /// One side starts a Logout.
    Logout,
    /// One side writes an old order again, marked as a possible duplicate.
    Duplicate,
}

const FAULTS: [Fault; 6] = [
    Fault::Cut,
    Fault::Drop,
    Fault::Stall,
    Fault::ClockStep,
    Fault::Logout,
    Fault::Duplicate,
];

#[derive(Debug, Clone)]
pub struct Plan {
    pub begin_string: &'static [u8],
    pub initiator_heartbeat_ms: u64,
    pub acceptor_heartbeat_ms: u64,
    /// The share of the random actions that are faults, in percent.
    pub fault_percent: u64,
    weights: [u64; 6],
    /// The faults happen in the first `fault_ms`.
    pub fault_ms: u64,
    /// The time after the faults. The sessions must recover in it.
    pub quiet_ms: u64,
}

impl Plan {
    pub fn from_seed(rng: &mut Rng) -> Self {
        let pick = |rng: &mut Rng, list: &[u64]| list[rng.index(list.len())];
        let initiator_heartbeat_ms = pick(rng, &HEARTBEATS_MS);
        let acceptor_heartbeat_ms = pick(rng, &HEARTBEATS_MS);
        let version = rng.index(VERSIONS.len());
        Self {
            begin_string: VERSIONS[version],
            initiator_heartbeat_ms,
            acceptor_heartbeat_ms,
            fault_percent: rng.range(0, 20),
            weights: [
                rng.range(0, 3),
                rng.range(0, 3),
                rng.range(0, 3),
                rng.range(0, 3),
                rng.range(0, 3),
                rng.range(0, 3),
            ],
            fault_ms: rng.range(60_000, 300_000),
            quiet_ms: 240_000 + 40 * initiator_heartbeat_ms,
        }
    }

    /// Chooses a fault with the weights of this plan. Returns `None` when
    /// every weight is 0.
    pub fn pick_fault(&self, rng: &mut Rng) -> Option<Fault> {
        let total: u64 = self.weights.iter().sum();
        if total == 0 {
            return None;
        }
        let mut roll = rng.below(total);
        for (fault, weight) in FAULTS.iter().zip(self.weights) {
            if roll < weight {
                return Some(*fault);
            }
            roll -= weight;
        }
        None
    }
}
