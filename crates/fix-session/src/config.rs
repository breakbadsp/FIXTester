/// Which side opens the connection and sends the first Logon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Initiator,
    Acceptor,
}

/// The settings of one session.
#[derive(Debug, Clone)]
pub struct Config {
    pub role: Role,
    /// `FIX.4.2`, `FIX.4.3`, or `FIX.4.4`.
    pub begin_string: Vec<u8>,
    pub sender_comp_id: Vec<u8>,
    pub target_comp_id: Vec<u8>,
    /// The initiator sends this in Logon. The acceptor uses the value that
    /// the initiator sends.
    pub heartbeat_interval_ms: u64,
    /// The initiator sends ResetSeqNumFlag=Y in Logon and starts both
    /// sequences at 1.
    pub reset_on_logon: bool,
    pub username: Option<Vec<u8>>,
    pub password: Option<Vec<u8>>,
    /// The largest difference between SendingTime and the local clock.
    pub sending_time_tolerance_ms: u64,
    /// The time to wait for a Logon or a Logout reply.
    pub logon_timeout_ms: u64,
    pub logout_timeout_ms: u64,
    /// The longest message that the session sends, in bytes.
    pub message_len_max: usize,
    /// The most output bytes that wait for the host to take them.
    pub output_len_max: usize,
}

impl Config {
    /// A configuration with the usual values: a 30 s heartbeat, a 120 s
    /// SendingTime tolerance, 10 s for Logon and Logout replies, 16 KiB
    /// messages, and 1 MiB of output.
    pub fn new(role: Role, begin_string: &[u8], sender: &[u8], target: &[u8]) -> Self {
        Self {
            role,
            begin_string: begin_string.to_vec(),
            sender_comp_id: sender.to_vec(),
            target_comp_id: target.to_vec(),
            heartbeat_interval_ms: 30_000,
            reset_on_logon: false,
            username: None,
            password: None,
            sending_time_tolerance_ms: 120_000,
            logon_timeout_ms: 10_000,
            logout_timeout_ms: 10_000,
            message_len_max: 16 * 1024,
            output_len_max: 1024 * 1024,
        }
    }

    /// Returns true when the version has Username(553) and Password(554) in
    /// Logon: FIX 4.3 and later.
    pub(crate) fn has_logon_credentials(&self) -> bool {
        self.begin_string.as_slice() != b"FIX.4.2"
    }
}
