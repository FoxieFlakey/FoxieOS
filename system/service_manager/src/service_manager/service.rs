use std::{
    cmp,
    collections::VecDeque,
    sync::{Arc, oneshot},
    time::{Duration, Instant},
};

use foxie_base::{
    sandboxer::{ExitReason, IProcessHandle, ISandbox},
    util::listeners::Listeners,
};
use libbinder::object::B;
use nix::{
    sys::{eventfd::EventFd, signal::Signal, timerfd::TimerFd},
    unistd::{Pid, Uid},
};
use parking_lot::Mutex;
use thiserror::Error;

use crate::service_manager::{self, Request, SINGLETON};

pub struct Config {
    pub name: String,
    pub binary: String,
    pub arg0: Option<String>,
    pub argv: Vec<String>,
    pub uid: Uid,

    pub sandbox_profile: foxie_base::sandboxer::Profile,

    // What is delay between respawning
    pub retry_delay: Duration,

    // How many retries can occur within last retry_window duration
    // before considred failed
    pub retry_count: u16,
    pub retry_window: Duration,

    pub graceful_signal: Signal,

    // Capped to be 5 secs long
    pub graceful_delay: Duration,
    pub tty: Option<String>,
}

impl Config {
    pub fn sanify(&mut self) {
        self.graceful_delay = cmp::min(
            cmp::max(self.graceful_delay, Duration::from_millis(200)),
            Duration::from_secs(5),
        );

        self.retry_delay = cmp::min(
            cmp::max(self.retry_delay, Duration::from_millis(200)),
            Duration::from_secs(10),
        );

        self.retry_count = self.retry_count.max(1);
        self.retry_window = cmp::min(
            cmp::max(self.retry_window, Duration::from_secs(10)),
            Duration::from_mins(5),
        );
    }
}

pub struct Service {
    pub(super) id: u32,
    // .0 is the state itself, and listeners for changes in service
    // NOTE: The service will be 'locked' do not call into service
    pub(super) state: Mutex<(ServiceStateFull, Listeners<()>)>,
}

pub(super) enum ServiceStateFull {
    Starting {
        config: Config,
        crash_detail: CrashDetail,
        sandbox: Arc<B<dyn ISandbox>>,
        handle: Arc<B<dyn IProcessHandle>>,

        // Services can notify via this if they're
        // ready or started or without support already
        // pre-fired
        notifier: EventFd,
        exited_notifier: Arc<EventFd>,
    },
    Started {
        config: Config,
        crash_detail: CrashDetail,
        sandbox: Arc<B<dyn ISandbox>>,
        handle: Arc<B<dyn IProcessHandle>>,
        exited_notifier: Arc<EventFd>,
    },
    Stopping {
        config: Config,
        crash_detail: CrashDetail,
        sandbox: Arc<B<dyn ISandbox>>,
        handle: Arc<B<dyn IProcessHandle>>,

        // When this timer triggered, the system will trigger
        // hard sigkill
        kill_timer: TimerFd,
        exited_notifier: Arc<EventFd>,
        start_on_stop: bool,
        stop_requested_at: Instant,
    },
    Stopped {
        config: Config,
        crash_detail: CrashDetail,
    },
    Crashed {
        config: Config,
        crash_detail: CrashDetail,
        exit_reason: ExitReason,
    },
    WaitingToRestart {
        config: Config,
        crash_detail: CrashDetail,
        exit_reason: ExitReason,
        retry_timer: TimerFd,
    },
    // Service no longer exists
    Deleted,
}

#[derive(Clone)]
pub struct CrashDetail {
    crashes: VecDeque<Instant>,
    window: Duration,
    max_count: u16,
}

impl CrashDetail {
    fn new(window: Duration, max_count: u16) -> Self {
        Self {
            crashes: VecDeque::new(),
            window,
            max_count,
        }
    }

    fn drop_olds(&mut self) {
        self.crashes.retain(|x| x.elapsed() < self.window);
    }

    pub(super) fn service_just_crashed(&mut self) {
        self.drop_olds();
        self.crashes.push_back(Instant::now());
        if self.crashes.len() > self.max_count.into() {
            self.crashes.pop_front();
        }
    }

    pub fn crash_count(&self) -> u16 {
        let count = self
            .crashes
            .iter()
            .filter(|x| x.elapsed() < self.window)
            .count();

        assert!(count <= u16::MAX.into());
        count as u16
    }

    pub fn is_allowed_to_restart(&self) -> bool {
        self.crash_count() < self.max_count.into()
    }
}

#[derive(Debug, Error)]
pub enum RequestError {
    #[error("Service is deleted")]
    ServiceIsDeleted,
}

impl Service {
    pub fn new(mut config: Config) -> Arc<Self> {
        config.sanify();

        let mut manager_state = SINGLETON
            .get()
            .expect("Manager isnt initialized")
            .state
            .lock();
        let svc = Arc::new(Self {
            id: manager_state.free_id,
            state: Mutex::new((
                ServiceStateFull::Stopped {
                    crash_detail: CrashDetail::new(config.retry_window, config.retry_count),
                    config,
                },
                Listeners::new(),
            )),
        });
        manager_state.free_id += 1;
        assert!(
            manager_state.services.insert(svc.id, svc.clone()).is_none(),
            "There cannot be another service with same ID"
        );
        svc
    }

    pub fn start(self: &Arc<Service>) -> Result<(), RequestError> {
        let (sender, receiver) = oneshot::channel();
        service_manager::get_manager().enqueue_req(Request::Start(
            self.clone(),
            Box::new(move |x| sender.send(x).unwrap()),
        ));

        receiver.recv().expect("Sender cant be dead")
    }

    pub fn stop(self: &Arc<Service>) -> Result<(), RequestError> {
        let (sender, receiver) = oneshot::channel();
        service_manager::get_manager().enqueue_req(Request::Stop(
            self.clone(),
            Box::new(move |x| sender.send(x).unwrap()),
        ));

        receiver.recv().expect("Sender cant be dead")
    }

    #[expect(unused)]
    pub fn status(&self) -> ServiceState {
        match &self.state.lock().0 {
            ServiceStateFull::Starting { handle, .. } => {
                let pid = handle.get_pid().expect("Cannot get pid of process");
                ServiceState::Starting(pid.0)
            }
            ServiceStateFull::Started { handle, .. } => {
                let pid = handle.get_pid().expect("Cannot get pid of process");
                ServiceState::Started(pid.0)
            }
            ServiceStateFull::Stopping { handle, .. } => {
                let pid = handle.get_pid().expect("Cannot get pid of process");
                ServiceState::Stopping(pid.0)
            }
            ServiceStateFull::Stopped { .. } => ServiceState::Stopped,
            ServiceStateFull::Crashed { exit_reason, .. } => ServiceState::Crashed {
                reason: *exit_reason,
            },
            ServiceStateFull::WaitingToRestart { .. } => ServiceState::WaitingToRestart,
            ServiceStateFull::Deleted => panic!("Service is deleted, cannot be used"),
        }
    }
}

#[expect(unused)]
pub enum ServiceState {
    Starting(Pid),
    Started(Pid),
    Stopping(Pid),
    Stopped,
    WaitingToRestart,
    Crashed { reason: ExitReason },
}
