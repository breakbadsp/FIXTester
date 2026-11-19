//! The world: two hosts joined by two pipes, a simulated clock, and the
//! random faults and traffic. [`World::run`] is the host loop for both sides.

use fix_codec::UtcMillis;
use fix_session::{Config, Role, State};

use crate::host::{Delivered, Host, Stats};
use crate::pipe::{Item, Pipe};
use crate::plan::{Fault, Plan};
use crate::rng::Rng;

/// The simulation starts here: 2023-11-14T22:13:20Z.
pub const START: UtcMillis = UtcMillis(1_700_000_000_000);
const TOLERANCE_MS: u64 = 120_000;
/// The largest offset of a local clock. With the longest stall, a received
/// SendingTime stays inside the tolerance.
const SKEW_LIMIT_MS: u64 = 15_000;
const CLOCK_STEP_MS: u64 = 5_000;
const STALL_MS: (u64, u64) = (1_000, 60_000);
const RECONNECT_MS_MAX: u64 = 3_000;
const STEPS_MAX: u64 = 2_000_000;
const SAME_TIME_STEPS_MAX: u64 = 1_000;
const INITIATOR: usize = 0;

/// Settings of a run that the seed does not decide.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Let the message-loss fault hit a ResendRequest or a gap fill. The
    /// session sends a new ResendRequest after one heartbeat interval.
    pub drop_recovery: bool,
}

/// What a run did. For the summary of the bin.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub seed: u64,
    /// The statistics of the initiator and of the acceptor.
    pub stats: [Stats; 2],
    pub cuts: u64,
    pub stalls: u64,
    pub clock_steps: u64,
    pub logouts: u64,
    pub duplicates: u64,
    pub refused_sends: u64,
    pub steps: u64,
    pub simulated_ms: u64,
}

#[derive(Debug)]
pub struct World {
    rng: Rng,
    now: UtcMillis,
    /// Index 0 is the initiator, 1 the acceptor.
    hosts: [Host; 2],
    /// `pipes[i]` carries the bytes that host `i` writes to the other host.
    pipes: [Pipe; 2],
    plan: Plan,
    fault_end: UtcMillis,
    liveness_from: UtcMillis,
    traffic_end: UtcMillis,
    end: UtcMillis,
    next_action: UtcMillis,
    connect_at: Option<UtcMillis>,
    close_at: [Option<UtcMillis>; 2],
    next_order: u64,
    same_time_steps: u64,
    report: Report,
}

fn config(plan: &Plan, role: Role) -> Config {
    let (own, peer, heartbeat): (&[u8], &[u8], u64) = match role {
        Role::Initiator => (b"INIT", b"ACCEPT", plan.initiator_heartbeat_ms),
        Role::Acceptor => (b"ACCEPT", b"INIT", plan.acceptor_heartbeat_ms),
    };
    let mut config = Config::new(role, plan.begin_string, own, peer);
    config.heartbeat_interval_ms = heartbeat;
    config.sending_time_tolerance_ms = TOLERANCE_MS;
    config.username = Some(b"user".to_vec());
    config.password = Some(b"pass".to_vec());
    config
}

impl World {
    pub fn new(seed: u64, options: Options) -> Self {
        let mut rng = Rng::new(seed);
        let plan = Plan::from_seed(&mut rng);
        let skews = [rng.signed(SKEW_LIMIT_MS), rng.signed(SKEW_LIMIT_MS)];
        let hosts = [
            Host::new(
                config(&plan, Role::Initiator),
                skews[0],
                options.drop_recovery,
            ),
            Host::new(
                config(&plan, Role::Acceptor),
                skews[1],
                options.drop_recovery,
            ),
        ];
        let fault_end = UtcMillis(START.0 + plan.fault_ms);
        let connect_at = UtcMillis(START.0 + rng.range(0, 100));
        Self {
            now: START,
            hosts,
            pipes: [Pipe::default(), Pipe::default()],
            fault_end,
            liveness_from: UtcMillis(fault_end.0 + plan.quiet_ms / 2),
            traffic_end: UtcMillis(fault_end.0 + plan.quiet_ms * 3 / 4),
            end: UtcMillis(fault_end.0 + plan.quiet_ms),
            next_action: START,
            connect_at: Some(connect_at),
            close_at: [None; 2],
            next_order: 1,
            same_time_steps: 0,
            report: Report {
                seed,
                ..Report::default()
            },
            plan,
            rng,
        }
    }

    pub fn now(&self) -> UtcMillis {
        self.now
    }

    /// Runs to the end of the quiet period, then checks the final state.
    pub fn run(&mut self) -> Result<Report, String> {
        while self.now < self.end {
            self.advance()?;
            self.step()?;
        }
        self.settle()?;
        self.check_final_state()?;
        self.report.stats = [self.hosts[0].stats.clone(), self.hosts[1].stats.clone()];
        self.report.simulated_ms = self.now.0 - START.0;
        Ok(self.report.clone())
    }

    /// Moves the clock to the next time when something is due.
    fn advance(&mut self) -> Result<(), String> {
        let mut next = self.end;
        let pipe_times = self.pipes.iter().filter_map(Pipe::next_at);
        let host_times = self.hosts.iter().filter_map(Host::deadline);
        let other_times = self.close_at.iter().flatten().chain(&self.connect_at);
        let action = (self.now < self.traffic_end).then_some(self.next_action);
        for time in pipe_times
            .chain(host_times)
            .chain(other_times.copied())
            .chain(action)
        {
            next = next.min(time);
        }
        if next <= self.now {
            self.same_time_steps += 1;
            if self.same_time_steps > SAME_TIME_STEPS_MAX {
                return Err(format!(
                    "no progress: something is due at {} ms but is never done",
                    next.0.saturating_sub(START.0)
                ));
            }
        } else {
            self.same_time_steps = 0;
        }
        self.now = self.now.max(next);
        self.report.steps += 1;
        if self.report.steps > STEPS_MAX {
            return Err("too many steps".into());
        }
        Ok(())
    }

    fn step(&mut self) -> Result<(), String> {
        if self.now >= self.fault_end {
            self.hosts
                .iter_mut()
                .for_each(|host| host.drop_armed = false);
        }
        self.deliver_due()?;
        self.run_timers()?;
        self.run_closes();
        self.maybe_connect()?;
        self.run_action()
    }

    fn side_order(&mut self) -> [usize; 2] {
        if self.rng.chance(50) { [0, 1] } else { [1, 0] }
    }

    fn deliver_due(&mut self) -> Result<(), String> {
        for from in self.side_order() {
            while let Some(item) = self.pipes[from].pop_due(self.now) {
                let to = 1 - from;
                match item {
                    Item::Bytes(bytes) => self.deliver_bytes(to, &bytes)?,
                    Item::Eof => self.close_host(to, false),
                }
            }
        }
        Ok(())
    }

    fn deliver_bytes(&mut self, to: usize, bytes: &[u8]) -> Result<(), String> {
        if !self.hosts[to].connected {
            return Ok(());
        }
        let delivered = self.hosts[to].receive(self.now, bytes)?;
        for message in &delivered {
            self.check_delivered(to, message)?;
        }
        self.pump(to)
    }

    fn run_timers(&mut self) -> Result<(), String> {
        for side in self.side_order() {
            if self.hosts[side]
                .deadline()
                .is_some_and(|due| due <= self.now)
            {
                self.hosts[side].timer(self.now);
                self.pump(side)?;
            }
        }
        Ok(())
    }

    fn run_closes(&mut self) {
        for side in 0..2 {
            if self.close_at[side].is_some_and(|at| at <= self.now) {
                self.close_host(side, false);
            }
        }
    }

    /// Opens a new connection when both sides are disconnected.
    fn maybe_connect(&mut self) -> Result<(), String> {
        if self.hosts.iter().any(|host| host.connected) {
            self.connect_at = None;
            return Ok(());
        }
        let delay = self.rng.range(0, RECONNECT_MS_MAX);
        let at = *self.connect_at.get_or_insert(UtcMillis(self.now.0 + delay));
        if at > self.now {
            return Ok(());
        }
        self.connect_at = None;
        self.pipes = [Pipe::default(), Pipe::default()];
        for side in [1, INITIATOR] {
            self.hosts[side].connect(self.now);
            self.pump(side)?;
        }
        Ok(())
    }

    /// Takes the output of host `side`, writes it to the wire, and closes the
    /// connection when the session asks for it.
    fn pump(&mut self, side: usize) -> Result<(), String> {
        let host = &mut self.hosts[side];
        host.flush(self.now, &mut self.pipes[side], &mut self.rng)?;
        host.check_numbers()?;
        if host.session.disconnect_requested() {
            self.close_host(side, true);
        }
        Ok(())
    }

    /// Closes the connection on host `side`. With `notify`, the other side
    /// reads the end of the stream after the bytes that are in flight.
    fn close_host(&mut self, side: usize, notify: bool) {
        self.close_at[side] = None;
        if !self.hosts[side].connected {
            return;
        }
        self.hosts[side].disconnect(self.now);
        if notify {
            self.pipes[side].close(self.now, &mut self.rng);
        }
        self.pipes[1 - side].kill();
    }

    fn run_action(&mut self) -> Result<(), String> {
        if self.now < self.next_action || self.now >= self.traffic_end {
            return Ok(());
        }
        self.next_action = UtcMillis(self.now.0 + self.rng.range(20, 2_000));
        let fault = if self.now < self.fault_end && self.rng.chance(self.plan.fault_percent) {
            self.plan.pick_fault(&mut self.rng)
        } else {
            None
        };
        match fault {
            Some(fault) => self.inject(fault),
            None => self.send_traffic(),
        }
    }

    fn send_traffic(&mut self) -> Result<(), String> {
        let side = usize::from(self.rng.chance(50));
        for _ in 0..self.rng.range(1, 3) {
            let id = self.next_order;
            self.next_order += 1;
            let Some(seq) = self.hosts[side].send_order(self.now, id)? else {
                self.report.refused_sends += 1;
                continue;
            };
            self.pump(side)?;
            let sent = self.hosts[side].sent.get(&seq);
            if sent.map(|sent| sent.order) != Some(Some(id)) {
                return Err(format!(
                    "send_app returned MsgSeqNum {seq}, not the order {id}"
                ));
            }
        }
        Ok(())
    }

    fn inject(&mut self, fault: Fault) -> Result<(), String> {
        let side = usize::from(self.rng.chance(50));
        match fault {
            Fault::Cut => self.cut(),
            Fault::Drop => self.hosts[side].drop_armed = true,
            Fault::Stall => {
                self.report.stalls += 1;
                let until = self.now.0 + self.rng.range(STALL_MS.0, STALL_MS.1);
                self.pipes[side].stall(UtcMillis(until));
            }
            Fault::ClockStep => self.step_clock(side),
            Fault::Logout => {
                if self.hosts[side].session.state() == State::Active {
                    self.report.logouts += 1;
                    self.hosts[side].logout(self.now);
                    self.pump(side)?;
                }
            }
            Fault::Duplicate => {
                let host = &mut self.hosts[side];
                if host.duplicate_order(self.now, &mut self.pipes[side], &mut self.rng)? {
                    self.report.duplicates += 1;
                }
            }
        }
        Ok(())
    }

    /// Both sides lose the connection after a short delay. Bytes in flight
    /// are lost.
    fn cut(&mut self) {
        self.report.cuts += 1;
        for index in 0..2 {
            self.pipes[index].kill();
            if self.hosts[index].connected && self.close_at[index].is_none() {
                let delay = self.rng.range(1, 500);
                self.close_at[index] = Some(UtcMillis(self.now.0 + delay));
            }
        }
    }

    fn step_clock(&mut self, side: usize) {
        self.report.clock_steps += 1;
        let limit = i64::try_from(SKEW_LIMIT_MS).unwrap_or(0);
        let skew = self.hosts[side].skew_ms + self.rng.signed(CLOCK_STEP_MS);
        self.hosts[side].skew_ms = skew.clamp(-limit, limit);
    }

    /// Delivers everything that is in flight, without running timers, so that
    /// the sequence numbers can be compared.
    fn settle(&mut self) -> Result<(), String> {
        while let Some(at) = self.pipes.iter().filter_map(Pipe::next_at).min() {
            self.now = self.now.max(at);
            self.report.steps += 1;
            if self.report.steps > STEPS_MAX {
                return Err("too many steps while settling".into());
            }
            self.deliver_due()?;
        }
        Ok(())
    }

    /// A delivered message must come from the peer, in order, once.
    fn check_delivered(&mut self, side: usize, message: &Delivered) -> Result<(), String> {
        let seq = message.seq;
        let origin = self.hosts[1 - side].sent.get(&seq);
        let genuine =
            origin.is_some_and(|sent| sent.kind == message.kind && sent.order == message.order);
        if !genuine {
            return Err(format!(
                "host {side} delivered {message:?}, but the peer sent {origin:?} as {seq}"
            ));
        }
        let host = &mut self.hosts[side];
        if seq <= host.last_delivered {
            return Err(format!(
                "host {side} delivered MsgSeqNum {seq} after {}",
                host.last_delivered
            ));
        }
        host.last_delivered = seq;
        if message
            .order
            .is_some_and(|order| !host.delivered_orders.insert(order))
        {
            return Err(format!(
                "host {side} delivered order {:?} twice",
                message.order
            ));
        }
        Ok(())
    }

    fn check_final_state(&self) -> Result<(), String> {
        for (side, host) in self.hosts.iter().enumerate() {
            if host.session.state() != State::Active {
                return Err(format!(
                    "host {side} is {:?} at the end, not Active",
                    host.session.state()
                ));
            }
        }
        for side in 0..2 {
            let target = self.hosts[side].session.sequence_numbers().next_target;
            let sender = self.hosts[1 - side].session.sequence_numbers().next_sender;
            if target != sender {
                return Err(format!(
                    "host {side} expects MsgSeqNum {target}, but the peer's next is {sender}"
                ));
            }
            self.check_no_late_order_lost(side)?;
        }
        Ok(())
    }

    /// An order that a side sent in the quiet period, with no fault left,
    /// must reach the peer.
    fn check_no_late_order_lost(&self, side: usize) -> Result<(), String> {
        let peer = &self.hosts[1 - side];
        for (seq, sent) in &self.hosts[side].sent {
            let lost = sent.order.is_some_and(|order| {
                sent.at >= self.liveness_from && !peer.delivered_orders.contains(&order)
            });
            if lost {
                return Err(format!(
                    "order {:?} (MsgSeqNum {seq}) of host {side}, sent at +{} ms in the quiet \
                     period, was never delivered",
                    sent.order,
                    sent.at.0 - START.0
                ));
            }
        }
        Ok(())
    }
}
