//! The FIX session layer as a state machine with no I/O and no clock.
//!
//! The host gives the session the time and each received message, and calls
//! [`Session::on_timer`] at [`Session::next_deadline`]. The session writes the
//! messages to send into its output, which the host takes with
//! [`Session::take_output`]. When the session wants the connection closed, it
//! sets [`Session::disconnect_requested`]; the host sends the output, closes
//! the connection, and calls [`Session::on_disconnected`].
//!
//! When the peer asks for a resend, the session sends a gap fill for the whole
//! range (requirement R17). It never sends old messages again.

mod config;
mod session;

pub use config::{Config, Role};
pub use session::{
    DisconnectReason, Disposition, Event, SendError, SequenceNumbers, Session, State,
};
