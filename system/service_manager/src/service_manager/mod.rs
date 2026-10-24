use std::{
    collections::{HashMap, VecDeque},
    mem,
    os::fd::AsFd,
    sync::{Arc, OnceLock, Weak},
    thread::{self, sleep},
    time::{Duration, Instant},
};

use anyhow::Context;
use foxie_base::{
    common::SerdeSignal,
    sandboxer::{self, ExitReason, IProcessHandle, ISandbox, SpawnArgs},
    util::{FnOnceObject, retry_eintr},
};
use libbinder::{Runtime, object::B};
use libbinder_basic::packetable::Serde;
use log::{error, info, warn};
use nix::{
    poll::PollTimeout,
    sys::{
        epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags},
        eventfd::EventFd,
        timer::{Expiration, TimerSetTimeFlags},
        timerfd::{ClockId, TimerFd, TimerFlags},
    },
    unistd::dup,
};
use parking_lot::{Condvar, Mutex, Once};

use crate::{
    constants::get_sandboxer,
    service_manager::service::{
        ActionAfterStopped, CrashDetail, RequestError, Service, ServiceStateFull,
    },
};

pub mod service;

pub struct ServiceManager {
    rt: Weak<Runtime>,
    state: Mutex<State>,
    shutted_down: Mutex<bool>,
    shutted_down_changed: Condvar,
    wake_up: EventFd,
}

enum Request {
    Start(
        Arc<Service>,
        Box<dyn FnOnce(Result<(), RequestError>) + Send + Sync>,
    ),
    Stop(
        Arc<Service>,
        Box<dyn FnOnce(Result<(), RequestError>) + Send + Sync>,
    ),
}

struct State {
    services: HashMap<u32, Arc<Service>>,
    shutting_down: bool,
    free_id: u32,
    reqs: VecDeque<Request>,
}

impl Drop for ServiceManager {
    fn drop(&mut self) {
        unreachable!("ServiceManager is singleton, cannot be dropped");
    }
}

static SINGLETON: OnceLock<ServiceManager> = OnceLock::new();
static ONCE_GUARD: Once = Once::new();

pub fn get_manager() -> &'static ServiceManager {
    SINGLETON
        .get()
        .expect("Service manager has to be initialized")
}

impl ServiceManager {
    pub fn shutdown(&self) {
        let mut state = self.state.lock();
        state.shutting_down = true;
        let mut reqs = mem::take(&mut state.reqs);
        for service in state.services.values() {
            reqs.push_back(Request::Stop(service.clone(), Box::new(|_| {})));
        }
        state.reqs = reqs;
        retry_eintr(|| self.wake_up.write(1)).expect("Cannot wake up manager thread");
        drop(state);

        self.shutted_down_changed
            .wait_while(&mut self.shutted_down.lock(), |&mut x| x == false);
    }

    fn enqueue_req(&self, req: Request) {
        self.state.lock().reqs.push_back(req);
        retry_eintr(|| self.wake_up.write(1)).expect("Cannot wake up manager thread");
    }
}

pub fn init(rt: Arc<Runtime>) {
    ONCE_GUARD.call_once(|| {
        SINGLETON
            .get_or_try_init(|| -> anyhow::Result<ServiceManager> {
                Ok(ServiceManager {
                    rt: Arc::downgrade(&rt),
                    state: Mutex::new(State {
                        services: HashMap::new(),
                        shutting_down: false,
                        free_id: 2398878931,
                        reqs: VecDeque::new(),
                    }),
                    wake_up: EventFd::new().context("Cannot create wakeup fd")?,
                    shutted_down: Mutex::new(false),
                    shutted_down_changed: Condvar::new(),
                })
            })
            .expect("Cannot initialize service manager");

        thread::spawn(manager_thread);
    });
}

fn manager_thread() {
    // A service has exited, or waited too long (only for stopping, starting, started state)
    const TYPE_SERVICE_DEATH: u64 = 0x0 << 32;

    // A service has started! (only for starting state)
    const TYPE_SERVICE_STARTED: u64 = 0x1 << 32;

    // A service is ready to be restarted (only for waiting to restart state)
    const TYPE_SERVICE_DO_RESTART: u64 = 0x2 << 32;

    // Watching for new requests
    const TYPE_WAKEUP_FD: u64 = 0x3 << 32;

    // A service need to be sigkill'ed to stop (only for stopping state)
    const TYPE_SERVICE_NEED_SIGKILL: u64 = 0x4 << 32;

    // A service timed out when starting
    const TYPE_SERVICE_START_TIMEOUT: u64 = 0x5 << 32;

    nix::sys::prctl::set_name(&c"Manager thread").unwrap();

    let epoll = retry_eintr(|| Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC))
        .expect("Cannot create epoll fd");
    let manager = SINGLETON.get().unwrap();

    retry_eintr(|| {
        epoll.add(
            manager.wake_up.as_fd(),
            EpollEvent::new(EpollFlags::EPOLLIN, TYPE_WAKEUP_FD),
        )
    })
    .expect("Cannot add wake up fd to epoll");

    // NOTE: DO NOT INCREASE THIS LIMIT! There no mechanism to handle
    // stale events
    let mut buf = [EpollEvent::empty(); 1];
    loop {
        let count = retry_eintr(|| epoll.wait(&mut buf, PollTimeout::NONE))
            .expect("Cannot epoll wait: {e}");

        fn do_starting(
            rt: &Weak<Runtime>,
            is_shutting_down: bool,
            epoll: &Epoll,
            service: &Service,
            crash_detail: CrashDetail,
            config: service::Config,
        ) -> ServiceStateFull {
            if is_shutting_down {
                warn!(
                    "Refused to start service '{}' because system shutting down",
                    config.name
                );

                return ServiceStateFull::Stopped {
                    config,
                    crash_detail,
                };
            }

            info!("Starting service '{}'", config.name);
            let ready_fd =
                retry_eintr(|| EventFd::new()).expect("Cannot create event fd for service ready");
            // SAFETY: Just dup the fd
            let ready_fd_for_service = unsafe {
                EventFd::from_owned_fd(
                    retry_eintr(|| dup(ready_fd.as_fd()))
                        .expect("Cannot create event fd for service ready"),
                )
            };

            let mut fd_args = Vec::new();
            if let Some(x) = config.ready_eventfd_arg.clone() {
                fd_args.push((x, ready_fd_for_service.into()));
            } else {
                // Service dont support ready notification
                retry_eintr(|| ready_fd.write(1)).expect("Cannot fire ready_fd");
            }

            let sandbox = get_sandboxer()
                .create_sandbox(Serde(sandboxer::Config {
                    uid: config.uid.as_raw(),
                    profile: config.sandbox_profile,
                }))
                .expect("Cannot send transaction to sandboxer")
                .expect("Cannot create sandbox");
            let handle = sandbox
                .spawn(SpawnArgs {
                    path: config.binary.as_str().into(),
                    argv: config.argv.clone(),
                    arg0: config.arg0.as_ref().map(|x| x.as_str().into()),
                    tty: config.tty.as_ref().map(|x| x.as_str().into()),
                    fd_args,
                })
                .expect("Cannot send transaction to sandboxer")
                .expect("Cannot start service");
            let exit_fd = Arc::new(
                retry_eintr(|| EventFd::new()).expect("Cannot create event fd for service exit"),
            );

            let cloned = exit_fd.clone();
            handle
                .on_death(FnOnceObject::new(rt.clone(), move || {
                    retry_eintr(|| cloned.write(1))
                        .expect("Cannot write to eventfd that process is dead");
                }))
                .expect("Cannot send transaction to sandboxer")
                .expect("Cannot register death listener for service's process");

            let start_timed_out =
                retry_eintr(|| TimerFd::new(ClockId::CLOCK_MONOTONIC, TimerFlags::TFD_CLOEXEC))
                    .expect("Cannot create timeout fd");
            retry_eintr(|| {
                start_timed_out.set(
                    Expiration::OneShot(config.start_timeout.into()),
                    TimerSetTimeFlags::empty(),
                )
            })
            .expect("Cannot arm start timeout timerfd");

            retry_eintr(|| {
                epoll.add(
                    start_timed_out.as_fd(),
                    EpollEvent::new(
                        EpollFlags::EPOLLIN,
                        TYPE_SERVICE_START_TIMEOUT | u64::from(service.id),
                    ),
                )
            })
            .expect("Cannot add start time out epoll");

            retry_eintr(|| {
                epoll.add(
                    exit_fd.as_fd(),
                    EpollEvent::new(
                        EpollFlags::EPOLLIN,
                        TYPE_SERVICE_DEATH | u64::from(service.id),
                    ),
                )
            })
            .expect("Cannot add exit fd to epoll");

            retry_eintr(|| {
                epoll.add(
                    ready_fd.as_fd(),
                    EpollEvent::new(
                        EpollFlags::EPOLLIN,
                        TYPE_SERVICE_STARTED | u64::from(service.id),
                    ),
                )
            })
            .expect("Cannot add ready fd to epoll");

            ServiceStateFull::Starting {
                config,
                crash_detail,
                sandbox,
                handle,
                notifier: ready_fd,
                exited_notifier: exit_fd,
                start_timed_out,
                starting_since: Instant::now(),
            }
        }

        fn do_restarting(
            is_shutting_down: bool,
            sandbox: Arc<B<dyn ISandbox>>,
            epoll: &Epoll,
            service: &Service,
            mut crash_detail: CrashDetail,
            reason: ExitReason,
            config: service::Config,
            starting_since: Option<Instant>,
            started_since: Option<Instant>,
        ) -> ServiceStateFull {
            // Kill all processes running
            sandbox
                .kill_all()
                .expect("Cannot send transaction to sandboxer")
                .expect("Cannot kill all processes in service on crash");
            crash_detail.service_just_crashed();

            let txt = match (starting_since, started_since) {
                (None, None) => String::new(),
                (None, Some(_)) => unreachable!(),
                (Some(starting_since), None) => format!(
                    " and service is running for {:.2} secs",
                    starting_since.elapsed().as_secs_f32()
                ),
                (Some(starting_since), Some(started_since)) => format!(
                    " and service is running for {:.2} secs and spent {:.2} secs in startup",
                    starting_since.elapsed().as_secs_f32(),
                    (starting_since.elapsed() - started_since.elapsed()).as_secs_f32()
                ),
            };

            if crash_detail.is_allowed_to_restart() && !is_shutting_down {
                error!(
                    "Service '{}' crashed with {reason}, retrying in {:.3} secs (fails {} times){}",
                    config.name,
                    config.retry_delay.as_secs_f32(),
                    crash_detail.crash_count(),
                    txt
                );

                let retry_timer =
                    retry_eintr(|| TimerFd::new(ClockId::CLOCK_MONOTONIC, TimerFlags::TFD_CLOEXEC))
                        .expect("Cannot create timerfd for scheduling restart");
                retry_eintr(|| {
                    retry_timer.set(
                        Expiration::OneShot(config.retry_delay.into()),
                        TimerSetTimeFlags::empty(),
                    )
                })
                .expect("Cannot arm timerfd");
                retry_eintr(|| {
                    epoll.add(
                        retry_timer.as_fd(),
                        EpollEvent::new(
                            EpollFlags::EPOLLIN,
                            TYPE_SERVICE_DO_RESTART | u64::from(service.id),
                        ),
                    )
                })
                .expect("Cannot add retry timer fd to epoll");

                ServiceStateFull::WaitingToRestart {
                    config,
                    crash_detail,
                    exit_reason: reason,
                    retry_timer,
                }
            } else {
                error!(
                    "Service '{}' crashed with {reason}, wont restarting{} (fails {} times)",
                    config.name,
                    if is_shutting_down {
                        "due system is shutting down"
                    } else {
                        ""
                    },
                    crash_detail.crash_count()
                );

                ServiceStateFull::Crashed {
                    config,
                    crash_detail,
                    exit_reason: reason,
                }
            }
        }

        fn do_stopping(
            config: service::Config,
            epoll: &Epoll,
            service: &Service,
            crash_detail: CrashDetail,
            sandbox: Arc<B<dyn ISandbox>>,
            handle: Arc<B<dyn IProcessHandle>>,
            exited_notifier: Arc<EventFd>,
            action_after_stopped: ActionAfterStopped,
            starting_since: Instant,
            started_since: Option<Instant>,
        ) -> ServiceStateFull {
            let stop_requested_at = Instant::now();
            info!(
                "Stopping '{}' (Phase 1, graceful signal {} and SIGKILL after {:.2} secs). Service been running for {:.2} secs{}",
                config.name,
                config.graceful_signal,
                config.graceful_delay.as_secs_f32(),
                starting_since.elapsed().as_secs_f32(),
                started_since
                    .map(|x| format!(
                        " and {:.2} secs starting",
                        (starting_since.elapsed() - x.elapsed()).as_secs_f32()
                    ))
                    .unwrap_or(String::new())
            );

            let kill_timer =
                retry_eintr(|| TimerFd::new(ClockId::CLOCK_MONOTONIC, TimerFlags::TFD_CLOEXEC))
                    .expect("Cannot create kill timerfd");
            retry_eintr(|| {
                kill_timer.set(
                    Expiration::OneShot(config.graceful_delay.into()),
                    TimerSetTimeFlags::empty(),
                )
            })
            .expect("Cannot arm kill timerfd");
            retry_eintr(|| {
                epoll.add(
                    kill_timer.as_fd(),
                    EpollEvent::new(
                        EpollFlags::EPOLLIN,
                        TYPE_SERVICE_NEED_SIGKILL | u64::from(service.id),
                    ),
                )
            })
            .expect("Cannot add kill timerfd to epoll");

            // Send graceful signal to process
            handle
                .send_signal(SerdeSignal(config.graceful_signal))
                .expect("Cannot send transaction to sandboxer")
                .expect("Cannot send graceful signal to main process");

            ServiceStateFull::Stopping {
                config,
                crash_detail,
                sandbox,
                handle,
                kill_timer,
                exited_notifier,
                action_after_stopped,
                stop_requested_at,
                starting_since,
                started_since,
            }
        }

        let mut manager_lock = manager.state.lock();
        let rt = &manager.rt;
        let is_shutting_down = manager_lock.shutting_down;
        for event in &buf[0..count] {
            let kind = event.data() & !u64::from(u32::MAX);
            if kind == TYPE_WAKEUP_FD {
                retry_eintr(|| manager.wake_up.read()).expect("Cannot read wakeup");

                // Is waken up to handle requests
                for req in manager_lock.reqs.drain(..) {
                    match req {
                        Request::Stop(service, fn_once) => {
                            let mut state = service.state.lock();
                            let next_state =
                                match mem::replace(&mut state.0, ServiceStateFull::Deleted) {
                                    ServiceStateFull::Starting {
                                        config,
                                        crash_detail,
                                        sandbox,
                                        handle,
                                        notifier,
                                        exited_notifier,
                                        start_timed_out,
                                        starting_since,
                                    } => {
                                        // Don't care about ready notifier anymore and start timeout
                                        retry_eintr(|| epoll.delete(notifier.as_fd()))
                                            .expect("Cannot delete ready notifier eventfd");
                                        retry_eintr(|| epoll.delete(start_timed_out.as_fd()))
                                            .expect("Cannot delete start timeout  timerfd");
                                        Ok(do_stopping(
                                            config,
                                            &epoll,
                                            &service,
                                            crash_detail,
                                            sandbox,
                                            handle,
                                            exited_notifier,
                                            ActionAfterStopped::Nothing,
                                            starting_since,
                                            None,
                                        ))
                                    }
                                    ServiceStateFull::Started {
                                        config,
                                        crash_detail,
                                        sandbox,
                                        handle,
                                        exited_notifier,
                                        starting_since,
                                        started_since,
                                    } => Ok(do_stopping(
                                        config,
                                        &epoll,
                                        &service,
                                        crash_detail,
                                        sandbox,
                                        handle,
                                        exited_notifier,
                                        ActionAfterStopped::Nothing,
                                        starting_since,
                                        Some(started_since),
                                    )),
                                    ServiceStateFull::WaitingToRestart {
                                        config,
                                        crash_detail,
                                        exit_reason,
                                        retry_timer,
                                    } => {
                                        retry_eintr(|| epoll.delete(retry_timer.as_fd()))
                                            .expect("Cannot remove retry timer");
                                        Ok(ServiceStateFull::Crashed {
                                            config,
                                            crash_detail,
                                            exit_reason,
                                        })
                                    }
                                    ServiceStateFull::Stopping {
                                        action_after_stopped:
                                            ActionAfterStopped::Start | ActionAfterStopped::Restart,
                                        config,
                                        crash_detail,
                                        sandbox,
                                        handle,
                                        kill_timer,
                                        exited_notifier,
                                        stop_requested_at,
                                        started_since,
                                        starting_since,
                                    } => Ok(ServiceStateFull::Stopping {
                                        config,
                                        crash_detail,
                                        sandbox,
                                        handle,
                                        kill_timer,
                                        exited_notifier,
                                        action_after_stopped: ActionAfterStopped::Nothing,
                                        stop_requested_at,
                                        started_since,
                                        starting_since,
                                    }),
                                    this @ (ServiceStateFull::Stopping {
                                        action_after_stopped: ActionAfterStopped::Nothing,
                                        ..
                                    }
                                    | ServiceStateFull::Stopped { .. }
                                    | ServiceStateFull::Crashed { .. }) => Ok(this),
                                    ServiceStateFull::Deleted => {
                                        Err(RequestError::ServiceIsDeleted)
                                    }
                                };

                            match next_state {
                                Ok(next_state) => {
                                    assert!(
                                        matches!(
                                            next_state,
                                            ServiceStateFull::Stopping {
                                                action_after_stopped: ActionAfterStopped::Nothing,
                                                ..
                                            } | ServiceStateFull::Crashed { .. }
                                                | ServiceStateFull::Stopped { .. },
                                        ),
                                        "Next state suppose be stopping with no other next state than nothing or crashed or stopped"
                                    );
                                    state.0 = next_state;
                                    let simplified = state.0.as_simplified();
                                    state.1.call_all(simplified);
                                    drop(state);

                                    fn_once(Ok(()));
                                }
                                Err(e) => {
                                    drop(state);
                                    fn_once(Err(e));
                                }
                            };
                        }
                        Request::Start(service, fn_once) => {
                            let mut state = service.state.lock();
                            let next_state = match mem::replace(
                                &mut state.0,
                                ServiceStateFull::Deleted,
                            ) {
                                ServiceStateFull::Stopped {
                                    config,
                                    crash_detail,
                                }
                                | ServiceStateFull::Crashed {
                                    config,
                                    crash_detail,
                                    ..
                                } => Ok(do_starting(
                                    rt,
                                    is_shutting_down,
                                    &epoll,
                                    &service,
                                    crash_detail,
                                    config,
                                )),
                                ServiceStateFull::WaitingToRestart {
                                    config,
                                    crash_detail,
                                    retry_timer,
                                    ..
                                } => {
                                    // User want to start early
                                    retry_eintr(|| epoll.delete(retry_timer.as_fd()))
                                        .expect("Cannot remove timerfd for restarting");

                                    Ok(do_starting(
                                        rt,
                                        is_shutting_down,
                                        &epoll,
                                        &service,
                                        crash_detail,
                                        config,
                                    ))
                                }
                                ServiceStateFull::Stopping {
                                    action_after_stopped:
                                        ActionAfterStopped::Nothing | ActionAfterStopped::Restart,
                                    config,
                                    crash_detail,
                                    sandbox,
                                    handle,
                                    kill_timer,
                                    exited_notifier,
                                    stop_requested_at,
                                    started_since,
                                    starting_since,
                                } => Ok(ServiceStateFull::Stopping {
                                    config,
                                    crash_detail,
                                    sandbox,
                                    handle,
                                    kill_timer,
                                    exited_notifier,
                                    action_after_stopped: ActionAfterStopped::Start,
                                    stop_requested_at,
                                    started_since,
                                    starting_since,
                                }),
                                this @ (ServiceStateFull::Stopping {
                                    action_after_stopped: ActionAfterStopped::Start,
                                    ..
                                }
                                | ServiceStateFull::Started { .. }
                                | ServiceStateFull::Starting { .. }) => Ok(this),
                                ServiceStateFull::Deleted => Err(RequestError::ServiceIsDeleted),
                            };

                            match next_state {
                                Ok(next_state) => {
                                    assert!(
                                        matches!(
                                            next_state,
                                            ServiceStateFull::Stopping {
                                                action_after_stopped: ActionAfterStopped::Restart
                                                    | ActionAfterStopped::Start,
                                                ..
                                            } | ServiceStateFull::Starting { .. }
                                                | ServiceStateFull::Started { .. }
                                                | ServiceStateFull::Stopped { .. },
                                        ),
                                        "Next state suppose be stoping with start_on_stop or starting or started"
                                    );
                                    state.0 = next_state;
                                    let simplified = state.0.as_simplified();
                                    state.1.call_all(simplified);
                                    drop(state);

                                    if is_shutting_down {
                                        fn_once(Err(RequestError::SystemIsShuttingDown));
                                    } else {
                                        fn_once(Ok(()));
                                    }
                                }
                                Err(e) => {
                                    drop(state);
                                    fn_once(Err(e));
                                }
                            };
                        }
                    }
                }

                continue;
            }

            let service = manager_lock
                .services
                .get(&((event.data() & u64::from(u32::MAX)) as u32))
                .expect("Service doesnt exists? but yet registered in epoll");
            let mut state = service.state.lock();
            if matches!(state.0, ServiceStateFull::Deleted) {
                // This event is irrelevant
                continue;
            }

            match kind {
                TYPE_SERVICE_START_TIMEOUT => {
                    let ServiceStateFull::Starting {
                        sandbox,
                        config,
                        crash_detail,
                        handle,
                        notifier,
                        exited_notifier,
                        start_timed_out,
                        starting_since,
                    } = mem::replace(&mut state.0, ServiceStateFull::Deleted)
                    else {
                        unreachable!("service should be just starting")
                    };
                    warn!(
                        "Service '{}' took too long to start >= {:.2} secs (starting since {:.2} secs)",
                        config.name,
                        config.start_timeout.as_secs_f32(),
                        starting_since.elapsed().as_secs_f32()
                    );

                    // Don't need to read the notifier, reading it would mean
                    // service manager acknowledge if its ready, but manager
                    // wont acknowledge it.
                    retry_eintr(|| epoll.delete(notifier.as_fd()))
                        .expect("Cannot remove ready notifier eventfd from epoll");
                    retry_eintr(|| epoll.delete(start_timed_out.as_fd()))
                        .expect("Cannot remove start timeout timerfd from epoll");
                    retry_eintr(|| start_timed_out.unset())
                        .expect("Cannot disarm the timeerfd for start timeout");

                    state.0 = do_stopping(
                        config,
                        &epoll,
                        service,
                        crash_detail,
                        sandbox,
                        handle,
                        exited_notifier,
                        ActionAfterStopped::Restart,
                        starting_since,
                        None,
                    );
                    let simplified = state.0.as_simplified();
                    state.1.call_all(simplified);
                }
                TYPE_SERVICE_NEED_SIGKILL => {
                    let ServiceStateFull::Stopping {
                        sandbox,
                        kill_timer,
                        config,
                        stop_requested_at,
                        ..
                    } = &state.0
                    else {
                        unreachable!("service should be just stopping")
                    };

                    warn!(
                        "Service '{}' being stubborn after {:.2} secs SIGKILL'ing it",
                        config.name,
                        stop_requested_at.elapsed().as_secs_f32()
                    );

                    retry_eintr(|| kill_timer.unset()).expect("Cannot disarm kill timer");
                    sandbox
                        .kill_all()
                        .expect("Cannot send transaction to sandboxer")
                        .expect("Cannot send SIGKILL to all service's processes");

                    // Later the state change will happen by the TYPE_SERVICE_DEATH
                }
                TYPE_SERVICE_STARTED => {
                    // A service has succesfully started
                    let ServiceStateFull::Starting {
                        config,
                        crash_detail,
                        sandbox,
                        handle,
                        notifier,
                        exited_notifier,
                        start_timed_out,
                        starting_since,
                    } = mem::replace(&mut state.0, ServiceStateFull::Deleted)
                    else {
                        unreachable!("Service has to be 'starting'")
                    };

                    info!(
                        "Service '{}' is started after {:.2} secs",
                        config.name,
                        starting_since.elapsed().as_secs_f32()
                    );

                    retry_eintr(|| epoll.delete(notifier.as_fd()))
                        .expect("Cannot remove started eventfd");

                    // NOTE: This is needed event we know its fired out. Mainly because
                    // there visible effects to outside of this process
                    retry_eintr(|| notifier.read()).expect("Cannot consume eventfd event");

                    retry_eintr(|| epoll.delete(start_timed_out.as_fd()))
                        .expect("Cannot remove start timeout timerfd");

                    state.0 = ServiceStateFull::Started {
                        config,
                        crash_detail,
                        sandbox,
                        handle,
                        exited_notifier,
                        started_since: Instant::now(),
                        starting_since,
                    };
                    let simplified = state.0.as_simplified();
                    state.1.call_all(simplified);
                }
                TYPE_SERVICE_DO_RESTART => {
                    let ServiceStateFull::WaitingToRestart {
                        config,
                        crash_detail,
                        retry_timer,
                        ..
                    } = mem::replace(&mut state.0, ServiceStateFull::Deleted)
                    else {
                        unreachable!("service should be just WaitingToRestart")
                    };

                    retry_eintr(|| epoll.delete(retry_timer.as_fd()))
                        .expect("Cannot remove retry timerfd on restart");

                    info!("Restarting '{}'", config.name);

                    state.0 =
                        do_starting(rt, is_shutting_down, &epoll, &service, crash_detail, config);
                    let simplified = state.0.as_simplified();
                    state.1.call_all(simplified);
                }
                TYPE_SERVICE_DEATH => {
                    let new_state = match mem::replace(&mut state.0, ServiceStateFull::Deleted) {
                        ServiceStateFull::Starting {
                            config,
                            crash_detail,
                            sandbox,
                            handle,
                            notifier,
                            exited_notifier,
                            start_timed_out,
                            starting_since,
                        } => {
                            // Service crashed
                            retry_eintr(|| epoll.delete(exited_notifier.as_fd()))
                                .expect("Cannot remove eventfd for exit notifier");
                            retry_eintr(|| epoll.delete(notifier.as_fd()))
                                .expect("Cannot remove eventfd for exit notifier");
                            retry_eintr(|| epoll.delete(start_timed_out.as_fd()))
                                .expect("Cannot remove timerfd for start timeout");
                            let reason = handle
                                .get_exit_reason()
                                .expect("Cannot send transaction to sandboxer")
                                .expect("Cannot get service exit reason")
                                .0
                                .expect("Service should already exited");

                            do_restarting(
                                is_shutting_down,
                                sandbox,
                                &epoll,
                                service,
                                crash_detail,
                                reason,
                                config,
                                Some(starting_since),
                                None,
                            )
                        }
                        ServiceStateFull::Started {
                            config,
                            crash_detail,
                            sandbox,
                            handle,
                            exited_notifier,
                            started_since,
                            starting_since,
                        } => {
                            // Service crashed
                            retry_eintr(|| epoll.delete(exited_notifier.as_fd()))
                                .expect("Cannot remove eventfd for exit notifier");
                            let reason = handle
                                .get_exit_reason()
                                .expect("Cannot send transaction to sandboxer")
                                .expect("Cannot get service exit reason")
                                .0
                                .expect("Service should already exited");

                            do_restarting(
                                is_shutting_down,
                                sandbox,
                                &epoll,
                                service,
                                crash_detail,
                                reason,
                                config,
                                Some(starting_since),
                                Some(started_since),
                            )
                        }
                        ServiceStateFull::Stopping {
                            config,
                            handle,
                            exited_notifier,
                            sandbox,
                            mut crash_detail,
                            kill_timer,
                            action_after_stopped,
                            stop_requested_at,
                            starting_since,
                            started_since,
                        } => {
                            // Service stopped either by kill_timer expiration or exited
                            retry_eintr(|| epoll.delete(exited_notifier.as_fd()))
                                .expect("Cannot remove eventfd for exit notifier");
                            retry_eintr(|| epoll.delete(kill_timer.as_fd()))
                                .expect("Cannot remove eventfd for kill timer");

                            let reason = handle
                                .get_exit_reason()
                                .expect("Cannot send transaction to sandboxer")
                                .expect("Cannot get service exit reason")
                                .0
                                .expect("Service should already exited");

                            // Kill all processes running
                            sandbox
                                .kill_all()
                                .expect("Cannot send transaction to sandboxer")
                                .expect("Cannot kill all processes in service on crash/exit");

                            let txt = match started_since {
                                None => format!(
                                    " and service is running for {:.2}",
                                    starting_since.elapsed().as_secs_f32()
                                ),
                                Some(started_since) => format!(
                                    " and service is running for {:.2} and spent {:.2} secs in startup",
                                    starting_since.elapsed().as_secs_f32(),
                                    (starting_since.elapsed() - started_since.elapsed())
                                        .as_secs_f32()
                                ),
                            };
                            if matches!(reason, ExitReason::Exit(0)) {
                                info!(
                                    "Service '{}' stopped with {reason} in {:.2} secs{txt}",
                                    config.name,
                                    stop_requested_at.elapsed().as_secs_f32()
                                );

                                match action_after_stopped {
                                    ActionAfterStopped::Nothing => ServiceStateFull::Stopped {
                                        config,
                                        crash_detail,
                                    },
                                    ActionAfterStopped::Restart => do_restarting(
                                        is_shutting_down,
                                        sandbox,
                                        &epoll,
                                        service,
                                        crash_detail,
                                        reason,
                                        config,
                                        Some(starting_since),
                                        started_since,
                                    ),
                                    ActionAfterStopped::Start => do_starting(
                                        rt,
                                        is_shutting_down,
                                        &epoll,
                                        &service,
                                        crash_detail,
                                        config,
                                    ),
                                }
                            } else {
                                warn!(
                                    "Service '{}' stopped with {reason} (did crashed during stopping) in {:.2} secs{txt}",
                                    config.name,
                                    stop_requested_at.elapsed().as_secs_f32()
                                );
                                crash_detail.service_just_crashed();
                                match action_after_stopped {
                                    ActionAfterStopped::Nothing => ServiceStateFull::Crashed {
                                        config,
                                        crash_detail,
                                        exit_reason: reason,
                                    },
                                    ActionAfterStopped::Restart => do_restarting(
                                        is_shutting_down,
                                        sandbox,
                                        &epoll,
                                        service,
                                        crash_detail,
                                        reason,
                                        config,
                                        Some(starting_since),
                                        started_since,
                                    ),
                                    ActionAfterStopped::Start => do_starting(
                                        rt,
                                        is_shutting_down,
                                        &epoll,
                                        &service,
                                        crash_detail,
                                        config,
                                    ),
                                }
                            }
                        }
                        ServiceStateFull::Stopped { .. }
                        | ServiceStateFull::WaitingToRestart { .. }
                        | ServiceStateFull::Crashed { .. }
                        | ServiceStateFull::Deleted => {
                            unreachable!("service should be either starting, started or stopping")
                        }
                    };
                    assert!(
                        matches!(
                            new_state,
                            ServiceStateFull::Crashed { .. }
                                | ServiceStateFull::Stopped { .. }
                                | ServiceStateFull::WaitingToRestart { .. }
                        ),
                        "Service must be crashed/stopped/waiting to restart"
                    );

                    state.0 = new_state;
                    let simplified = state.0.as_simplified();
                    state.1.call_all(simplified);
                }
                _ => unreachable!("Unknown kind"),
            }
        }

        if is_shutting_down {
            // keep checking if all services are down. no new starting can be added
            // if there started, enqueue generated Stop

            let mut reqs = mem::take(&mut manager_lock.reqs);
            let mut stop_count = 0;
            for svc in manager_lock.services.values() {
                match &svc.state.lock().0 {
                    // These would started so lets queue keep spamming stop
                    ServiceStateFull::Starting { .. }
                    | ServiceStateFull::Started { .. }
                    | ServiceStateFull::Stopping {
                        action_after_stopped:
                            ActionAfterStopped::Restart | ActionAfterStopped::Start,
                        ..
                    }
                    | ServiceStateFull::WaitingToRestart { .. } => {
                        reqs.push_back(Request::Stop(svc.clone(), Box::new(|_| {})));
                    }

                    // While these can be ignored because they're already way to stop
                    ServiceStateFull::Stopping {
                        action_after_stopped: ActionAfterStopped::Nothing,
                        ..
                    } => {}

                    // These mean service is completely stopped
                    ServiceStateFull::Stopped { .. }
                    | ServiceStateFull::Crashed { .. }
                    | ServiceStateFull::Deleted => {
                        stop_count += 1;
                    }
                }
            }

            manager_lock.reqs = reqs;

            if stop_count == manager_lock.services.len() {
                break;
            }
        }

        drop(manager_lock);
    }

    *manager.shutted_down.lock() = true;
    manager.shutted_down_changed.notify_all();

    loop {
        sleep(Duration::from_hours(1));
    }
}
