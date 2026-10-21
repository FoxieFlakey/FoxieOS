// Instead of managing services available in binder
// this one is managing lifecycle of services essentially
// the what systemd, openrc, etc do

use std::{
    cmp,
    collections::HashMap,
    ffi::CString,
    os::{
        fd::{AsFd, FromRawFd, IntoRawFd},
        linux::process::PidFd,
    },
    sync::{Arc, LazyLock, OnceLock, mpsc, oneshot},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use foxie_base::{
    common::SerdeSignal,
    sandboxer::{IProcessHandle, ISandbox, SpawnArgs},
};
use libbinder::object::B;
use libbinder_basic::packetable::Serde;
use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, PollTimeout, poll, ppoll},
    sys::{
        epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags},
        eventfd::{EfdFlags, EventFd},
        prctl,
        signal::{SigSet, Signal},
        timer::{Expiration, TimerSetTimeFlags},
        timerfd::{ClockId, TimerFd, TimerFlags},
        wait::WaitStatus,
    },
    unistd::{Pid, Uid},
};

use pidfd_util::PidFdExtFoxie;

struct Service {
    id: u64,
    sandbox: Arc<B<dyn ISandbox>>,
    main_pid: Option<(PidFd, Arc<B<dyn IProcessHandle>>, Pid)>,
    config: Config,
    state: State,

    death_count: u32,
    last_death: Instant,
}

#[derive(PartialEq, Eq)]
enum State {
    Stopped,
    Started,
    Dead,
}

impl Service {
    fn spawn(&mut self) -> anyhow::Result<()> {
        assert!(
            self.state != State::Started,
            "Cannot spawn service that is curently running"
        );

        let (pidfd, handle, pid) = self
            .sandbox
            .spawn(Serde(SpawnArgs {
                path: self.config.binary.clone(),
                argv: self.config.argv.clone(),
                arg0: self.config.arg0.clone(),
            }))
            .context("Cannot send transaction to sandboxer for spawning")?
            .context("Cannot spawn service into sandbox")?;

        // SAFETY: We just got it from std's PidFd
        self.main_pid = Some((
            unsafe { PidFd::from_raw_fd(pidfd.into_inner().into_raw_fd()) },
            handle,
            Pid::from_raw(pid),
        ));
        self.state = State::Started;
        Ok(())
    }

    fn stop_impl(&mut self) -> anyhow::Result<Option<PidFd>> {
        if self.state != State::Started {
            // Service already stopped
            return Ok(None);
        }

        let (pidfd, handle, pid) = self.main_pid.take().unwrap();

        match self.stop_impl_impl(&pidfd, &handle, pid) {
            Ok(()) => Ok(Some(pidfd)),
            Err(e) => {
                self.main_pid = Some((pidfd, handle, pid));
                Err(e)
            }
        }
    }

    fn stop_impl_impl(
        &mut self,
        pidfd: &PidFd,
        handle: &Arc<B<dyn IProcessHandle>>,
        pid: Pid,
    ) -> anyhow::Result<()> {
        let name = &self.config.name;

        // First try graceful kill
        let stop_time = Instant::now();
        let pid_str = format!("{}", pid);
        println!("service_manager: Stopping service '{name}' (pid {pid_str})");
        handle
            .send_signal(SerdeSignal(self.config.graceful_signal))
            .context("Cannot send transaction to sandboxer")?
            .with_context(|| {
                format!(
                    "Cannot send graceful signal {} to {pid_str}",
                    self.config.graceful_signal,
                )
            })?;

        // Wait either graceful wait delay passed or pidfd exited
        let mut pollfds = [PollFd::new(pidfd.as_fd(), PollFlags::POLLIN)];

        loop {
            let ret = ppoll(
                &mut pollfds,
                Some(self.config.graceful_delay.into()),
                Some(SigSet::all()),
            )
            .expect("Cannot ppoll for service to stop");

            if ret < 0 {
                continue;
            }

            if ret == 0 {
                // Timeout passed, does death killing anyway
                assert!(stop_time.elapsed() >= self.config.graceful_delay);
                break;
            }

            if !pollfds[0].revents().unwrap().is_empty() {
                break;
            } else {
                panic!("ppoll returns positive but pidfd isnt ready (only watching the pidfd)");
            }
        }

        // Either if service exited properly or graceful passed
        // kill everything in service sandbox.
        self.sandbox
            .kill_all()
            .context("Cannot send request to kill sandbox")?
            .context("Cannot kill all processes")?;
        self.state = State::Stopped;

        match pidfd
            .wait_for_termination_reason()
            .context("Cannot wait for termination status of service")
        {
            Ok(WaitStatus::Signaled(_, signal, ..)) => {
                println!(
                    "service_manager: Service '{name}' (pid {pid}) killed with {signal} in {:3.2} secs",
                    stop_time.elapsed().as_secs_f32()
                );
            }
            Ok(WaitStatus::Exited(_, code)) => {
                println!(
                    "service_manager: Service '{name}' (pid {pid}) stopped with code {code} in {:3.2} secs",
                    stop_time.elapsed().as_secs_f32()
                );
            }
            Ok(_) => panic!("unexpected wait status"),
            Err(e) => {
                panic!("Failed to wait on service pidfd: {}", e);
            }
        };
        Ok(())
    }
}

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
    pub retry_count: u32,
    pub retry_window: Duration,

    pub graceful_signal: Signal,

    // Capped to be 5 secs long
    pub graceful_delay: Duration,
}

pub enum Request {
    AddService(Config),
    Start(String),
    #[expect(unused)]
    Stop(String),
    #[expect(unused)]
    RemoveService(String),
}

static REQUEST: OnceLock<mpsc::Sender<(Request, Box<dyn FnOnce(anyhow::Result<()>) + Send>)>> =
    OnceLock::new();
static REQUEST_ENQUEUED: LazyLock<EventFd> = LazyLock::new(|| {
    EventFd::from_value_and_flags(0, EfdFlags::EFD_CLOEXEC)
        .expect("Cannot create eventfd for request enqueued")
});

pub fn send_request(req: Request) -> anyhow::Result<()> {
    let (sender, recv) = oneshot::channel();

    REQUEST
        .get()
        .unwrap()
        .send((req, Box::new(move |x| sender.send(x).unwrap())))
        .unwrap();
    REQUEST_ENQUEUED.write(1).unwrap();

    recv.recv().unwrap()
}

pub fn start_watcher() -> anyhow::Result<()> {
    let (sender, recv) = mpsc::channel();
    REQUEST.set(sender).unwrap();

    let epoll = Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC).context("Cannot create epoll fd")?;
    let retry_epoll = Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC)
        .context("Cannot create epoll fd for retries")?;
    thread::spawn(move || {
        prctl::set_name(&CString::new("Watcher").unwrap()).expect("Cannot rename watcher thread");

        let recv = recv;
        let mut active_services: HashMap<String, Service> = HashMap::new();
        let mut epoll_data_to_name_mapper: HashMap<u64, String> = HashMap::new();
        let mut epoll_data_to_retry_timerfd_mapper: HashMap<u64, TimerFd> = HashMap::new();
        let mut service_id_for_epoll_data = 0u64;

        loop {
            let mut pollfds = [
                PollFd::new(epoll.0.as_fd(), PollFlags::POLLIN),
                PollFd::new(retry_epoll.0.as_fd(), PollFlags::POLLIN),
                PollFd::new(REQUEST_ENQUEUED.as_fd(), PollFlags::POLLIN),
            ];

            loop {
                let Err(e) = poll(&mut pollfds, PollTimeout::NONE) else {
                    break;
                };

                if e == Errno::EINTR {
                    // Interrupted
                    continue;
                }
                Err::<(), _>(e).expect("Cannot poll");
            }

            // Handle deaths
            if !pollfds[0].revents().unwrap().is_empty() {
                // Some service exited!
                let mut events = [EpollEvent::empty(); 100];
                let count = epoll
                    .wait(&mut events, None::<PollTimeout>)
                    .expect("Cannot epoll on pifds");

                for event in &events[0..count] {
                    if event.events().intersects(EpollFlags::EPOLLHUP) {
                        // A service is dead, lets remove its pidfd from epoll and figure why its dead
                        let name = epoll_data_to_name_mapper
                            .get(&event.data())
                            .expect("epoll cannot contain unknown fd?");
                        let service = active_services
                            .get_mut(name)
                            .expect("epoll cannot contain unknown fd?");

                        let (pidfd, _, pid) = service
                            .main_pid
                            .take()
                            .expect("Service must have already its pidfd present");

                        match pidfd.get_termination_reason() {
                            Ok(WaitStatus::Signaled(_, signal, ..)) => {
                                println!(
                                    "service_manager: Service '{name}' (pid {pid}) killed with {signal} retrying in {} secs",
                                    service.config.retry_delay.as_secs()
                                );
                            }
                            Ok(WaitStatus::Exited(_, code)) => {
                                println!(
                                    "service_manager: Service '{name}' (pid {pid}) exited with code {code} retrying in {} secs",
                                    service.config.retry_delay.as_secs()
                                );
                            }
                            Ok(_) => panic!("unexpected wait status"),
                            Err(e) => {
                                panic!("Failed to wait on service pidfd: {}", e);
                            }
                        };

                        epoll
                            .delete(pidfd)
                            .expect("Cannot remove dead service pidfd");

                        service.state = State::Dead;
                        service.death_count += 1;
                        if service.last_death.elapsed() < service.config.retry_window
                            && service.death_count >= service.config.retry_count
                        {
                            println!(
                                "service_manager: Service '{name}' died too many time (>= {}) within {:3.2} secs. Won't restarting",
                                service.config.retry_count,
                                service.config.retry_window.as_secs_f32()
                            );
                        } else {
                            // Allowed to retry, then schedule a retry
                            let retry_timer =
                                TimerFd::new(ClockId::CLOCK_MONOTONIC, TimerFlags::TFD_CLOEXEC)
                                    .expect(
                                        "Cannot create timerfd for deadline on restarting service",
                                    );
                            retry_timer
                                .set(
                                    Expiration::OneShot(service.config.retry_delay.into()),
                                    TimerSetTimeFlags::empty(),
                                )
                                .expect("Cannot arm timer for retrying later");
                            retry_epoll
                                .add(
                                    retry_timer.as_fd(),
                                    EpollEvent::new(EpollFlags::EPOLLIN, service.id),
                                )
                                .expect("Cannot add timer to epoll fd for retry timers");
                            epoll_data_to_retry_timerfd_mapper.insert(service.id, retry_timer);
                        }
                    }
                }
            }

            // Handle retries
            if !pollfds[1].revents().unwrap().is_empty() {
                let mut events = [EpollEvent::empty(); 100];
                let count = retry_epoll
                    .wait(&mut events, None::<PollTimeout>)
                    .expect("Cannot epoll on pifds");

                for event in &events[0..count] {
                    let name = epoll_data_to_name_mapper
                        .get(&event.data())
                        .expect("epoll cannot contain unknown fd?");
                    let service = active_services
                        .get_mut(name)
                        .expect("epoll cannot contain unknown fd?");
                    let timer = epoll_data_to_retry_timerfd_mapper
                        .get(&event.data())
                        .expect("Epoll cannot contains unknown id");

                    retry_epoll
                        .delete(timer.as_fd())
                        .expect("Cannot remove retry timer fd from epoll");

                    if let Err(e) = service.spawn() {
                        println!(
                            "service_manager: Cannot respawn service retrying in {:3.2} secs: {e}",
                            service.config.retry_delay.as_secs_f32()
                        );
                    } else {
                        println!("service_manager: Respawned service '{name}'");
                        epoll_data_to_retry_timerfd_mapper.remove(&service.id);
                        epoll
                            .add(
                                service.main_pid.as_ref().unwrap().0.as_fd(),
                                EpollEvent::new(EpollFlags::EPOLLHUP, service.id),
                            )
                            .expect("Cannot add respawned service to epoll watch list");
                    }
                }
            }

            // Handle requests
            if !pollfds[2].revents().unwrap().is_empty() {
                // There incoming request :3
                REQUEST_ENQUEUED.read().unwrap();

                for (request, on_done) in recv.try_iter() {
                    let ret = match request {
                        Request::Start(name) => (|| {
                            let Some(service) = active_services.get_mut(&name) else {
                                bail!("Service named '{name}' dont exists")
                            };
                            println!("service_manager: Starting service '{name}'");

                            if service.state == State::Started {
                                // Service already started
                                return Ok(());
                            }

                            service.death_count = 0;
                            service.last_death = Instant::now();
                            service.spawn().context("Spawning service")?;
                            epoll
                                .add(
                                    service.main_pid.as_ref().unwrap().0.as_fd(),
                                    EpollEvent::new(
                                        EpollFlags::EPOLLIN | EpollFlags::EPOLLHUP,
                                        service.id,
                                    ),
                                )
                                .expect("Cannot add service's pidfd into epoll");
                            epoll_data_to_name_mapper.insert(service.id, name.clone());
                            Ok(())
                        })(),
                        Request::RemoveService(name) => (|| {
                            let Some(mut service) = active_services.remove(&name) else {
                                bail!("Service named '{name}' dont exists")
                            };
                            println!("service_manager: Removing service '{name}'");

                            if let Some(pid) = service.stop_impl()? {
                                // It was running, then remove it from epoll
                                epoll.delete(pid.as_fd()).expect("cannot remove from epoll");
                                epoll_data_to_name_mapper
                                    .remove(&service.id)
                                    .expect("There suppose be service here");
                                if let Some(x) =
                                    epoll_data_to_retry_timerfd_mapper.remove(&service.id)
                                {
                                    // Service was currently retrying, lets remove epoll for it
                                    retry_epoll
                                        .delete(x.as_fd())
                                        .expect("cannot remove service from retry epoll");
                                }
                            }

                            // Then remove it from registry
                            active_services.remove(&name);
                            Ok(())
                        })(),
                        Request::AddService(mut config) => (|| {
                            if active_services.contains_key(&config.name) {
                                bail!("Service named '{}' already existed", &config.name)
                            }
                            println!(
                                "service_manager: Adding service '{}' ({})",
                                config.name, config.binary,
                            );

                            // Set upper and lower limits on stuffs
                            config.graceful_delay = cmp::max(
                                cmp::min(config.graceful_delay, Duration::from_millis(200)),
                                Duration::from_secs(3),
                            );

                            config.retry_delay = cmp::max(
                                cmp::min(config.retry_delay, Duration::from_millis(200)),
                                Duration::from_secs(10),
                            );

                            config.retry_count = config.retry_count.max(1);
                            config.retry_window = cmp::max(
                                cmp::min(config.retry_window, Duration::from_secs(10)),
                                Duration::from_mins(5),
                            );

                            let sandbox = crate::constants::get_sandboxer()
                                .create_sandbox(Serde(foxie_base::sandboxer::Config {
                                    uid: config.uid.as_raw(),
                                    profile: config.sandbox_profile,
                                }))
                                .context("Cannot call to sandboxer service")?
                                .context("Cannot create sandbox")?;

                            let id = service_id_for_epoll_data;
                            let name = config.name.clone();
                            let mut service = Service {
                                id,
                                sandbox,
                                main_pid: None,
                                config,
                                death_count: 0,
                                last_death: Instant::now(),
                                state: State::Stopped,
                            };
                            service.spawn()?;

                            epoll
                                .add(
                                    service.main_pid.as_ref().unwrap().0.as_fd(),
                                    EpollEvent::new(
                                        EpollFlags::EPOLLIN | EpollFlags::EPOLLHUP,
                                        service.id,
                                    ),
                                )
                                .expect("Cannot add service's pidfd into epoll");
                            epoll_data_to_name_mapper.insert(id, name.clone());
                            active_services.insert(name, service);
                            service_id_for_epoll_data += 1;
                            Ok(())
                        })(),
                        Request::Stop(name) => (|| {
                            let Some(service) = active_services.get_mut(&name) else {
                                bail!("Service named '{name}' dont exists")
                            };

                            let Some(pid) = service.stop_impl()? else {
                                // Service already stopped
                                return Ok(());
                            };

                            // Service is entirely dead by this point
                            epoll.delete(pid).expect(
                                "Epoll failed to remove PID fd (there no sane recovery here)",
                            );
                            epoll_data_to_name_mapper
                                .remove(&service.id)
                                .expect("Service ID missing from epoll ID to name map");
                            Ok(())
                        })(),
                    };

                    on_done(ret);
                }
            }
        }
    });
    Ok(())
}
