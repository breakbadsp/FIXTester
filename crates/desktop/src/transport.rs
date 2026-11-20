mod peer;

use std::collections::BTreeMap;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use fix_session::{Role, SequenceNumbers, State};

use crate::model::{Application, Order, SESSIONS_MAX, execution::Report, grouped::GroupedOrder};
use crate::store::{SavedProfile, Store};
use crate::{Result, invalid};

const POLL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Starting,
    Listening,
    Link(State),
    Stopped,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Listening => "Listening",
            Self::Link(State::AwaitingLogon | State::LogonSent) => "Logon",
            Self::Link(State::Active) => "Active",
            Self::Link(State::LogoutSent | State::Disconnecting) => "Logout",
            Self::Link(State::Disconnected) | Self::Stopped => "Stopped",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Status {
    pub phase: Phase,
    pub seq: SequenceNumbers,
    pub notice: String,
    pub completed: u64,
    pub send_error: Option<String>,
    pub revision: u64,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            phase: Phase::Starting,
            seq: SequenceNumbers::default(),
            notice: String::new(),
            completed: 0,
            send_error: None,
            revision: 0,
        }
    }
}

#[derive(Debug)]
pub enum Send {
    Tags(Application),
    Order(Order),
    GroupedOrder(GroupedOrder),
    Execution(Report),
}

#[derive(Debug)]
struct SendRequest {
    message: Send,
    additional: String,
}

#[derive(Debug)]
struct Worker {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    send: SyncSender<SendRequest>,
    thread: JoinHandle<()>,
}

impl Worker {
    fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[derive(Debug, Default)]
pub struct Sessions {
    workers: BTreeMap<i64, Worker>,
}

impl Sessions {
    pub fn count(&self) -> usize {
        self.workers
            .values()
            .filter(|worker| !worker.thread.is_finished())
            .count()
    }

    pub fn status(&self, id: i64) -> Option<Status> {
        self.workers.get(&id).map(Worker::status)
    }

    pub fn is_running(&self, id: i64) -> bool {
        self.workers
            .get(&id)
            .is_some_and(|worker| !worker.thread.is_finished())
    }

    pub fn start(&mut self, store: &Store, saved: SavedProfile) -> Result<()> {
        self.require_stopped(saved.id)?;
        if self.count() == SESSIONS_MAX {
            return Err(invalid(
                "Eight sessions are already running. Stop one to start another.",
            ));
        }
        let checkpoint = store.checkpoint(saved.id)?;
        let open = store.connector();
        let id = saved.id;
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Status {
            seq: checkpoint.seq,
            ..Status::default()
        }));
        let (send, commands) = sync_channel(1);
        let control = Control {
            stop: Arc::clone(&stop),
            status: Arc::clone(&status),
            commands,
        };
        let thread = thread::Builder::new()
            .name(format!("fix-session-{id}"))
            .spawn(move || {
                let result = open().and_then(|mut store| run(&mut store, &saved, &control));
                control.update(|status| {
                    status.phase = Phase::Stopped;
                    if let Err(error) = result {
                        status.notice = error.to_string();
                    }
                });
            })?;
        self.workers.insert(
            id,
            Worker {
                stop,
                status,
                send,
                thread,
            },
        );
        Ok(())
    }

    pub fn stop(&self, id: i64) {
        if let Some(worker) = self.workers.get(&id) {
            worker.stop.store(true, Ordering::Relaxed);
        }
    }

    pub fn reset(&mut self, store: &Store, id: i64) -> Result<()> {
        self.require_stopped(id)?;
        store.arm_reset(id)?;
        self.workers.remove(&id);
        Ok(())
    }

    pub fn send(&self, id: i64, message: Send) -> Result<()> {
        self.send_with_additional_tags(id, message, "")
    }

    pub fn send_with_additional_tags(
        &self,
        id: i64,
        message: Send,
        additional: &str,
    ) -> Result<()> {
        if !additional.is_empty() && matches!(message, Send::Tags(_)) {
            return Err(invalid("Enter raw tags in the tag editor."));
        }
        let worker = self
            .workers
            .get(&id)
            .ok_or_else(|| invalid("Start the selected session."))?;
        let status = worker
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if worker.stop.load(Ordering::Relaxed) || status.phase != Phase::Link(State::Active) {
            return Err(invalid("The selected session is not active."));
        }
        // The acceptor changes phase under this lock before it drains commands.
        worker
            .send
            .try_send(SendRequest {
                message,
                additional: additional.to_owned(),
            })
            .map_err(|_| invalid("The session cannot accept another command."))
    }

    pub fn require_stopped(&self, id: i64) -> Result<()> {
        if self.is_running(id) {
            return Err(invalid("Stop the selected session first."));
        }
        Ok(())
    }
}

impl Drop for Sessions {
    fn drop(&mut self) {
        for worker in self.workers.values() {
            worker.stop.store(true, Ordering::Relaxed);
        }
        for worker in std::mem::take(&mut self.workers).into_values() {
            let _ = worker.thread.join();
        }
    }
}

struct Control {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    commands: Receiver<SendRequest>,
}

impl Control {
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn update(&self, change: impl FnOnce(&mut Status)) {
        change(
            &mut self
                .status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }
}

fn run(store: &mut Store, saved: &SavedProfile, control: &Control) -> Result<()> {
    match saved.profile.role {
        Role::Initiator => {
            let stream =
                TcpStream::connect_timeout(&saved.profile.address, Duration::from_secs(3))?;
            if !control.stopping() {
                peer::connect(stream, store, saved, control)?;
            }
            Ok(())
        }
        Role::Acceptor => listen(store, saved, control),
    }
}

fn listen(store: &mut Store, saved: &SavedProfile, control: &Control) -> Result<()> {
    let listener = TcpListener::bind(saved.profile.address)?;
    listener.set_nonblocking(true)?;
    while !control.stopping() {
        control.update(|status| status.phase = Phase::Listening);
        match listener.accept() {
            Ok((stream, _)) => {
                let result = peer::connect(stream, store, saved, control);
                if matches!(result, Err(crate::Error::Database(_))) {
                    return result;
                }
                control.update(|status| {
                    status.phase = Phase::Listening;
                    if let Err(error) = result {
                        status.notice = error.to_string();
                    }
                });
                while control.commands.try_recv().is_ok() {
                    control.update(|status| {
                        status.completed += 1;
                        status.notice = "Connection closed before the send command ran.".into();
                        status.send_error = Some(status.notice.clone());
                    });
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}
