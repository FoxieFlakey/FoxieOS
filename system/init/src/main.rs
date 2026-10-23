use std::{
    convert::Infallible,
    fs::{self, File, OpenOptions},
    os::fd::{AsFd, AsRawFd},
    path::Path,
    process::Command,
    sync::{
        LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, sleep},
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use command_fds::CommandFdExt;
use nix::{
    errno::Errno,
    fcntl::AT_FDCWD,
    mount::{MsFlags, mount},
    sys::{
        eventfd::EventFd,
        reboot::{RebootMode, reboot},
        signal::{Signal, kill},
        stat::{FchmodatFlags, Mode, fchmodat},
        wait::{WaitStatus, waitpid},
    },
    unistd::{Pid, dup, dup2_stderr, dup2_stdin, dup2_stdout, sync},
};
use parking_lot::{Condvar, Mutex};

// Whether the service manager intending to shutdown
static INTENDING_TO_SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn main() -> anyhow::Result<Infallible> {
    if Pid::this().as_raw() != 1 {
        bail!("Must run as PID 1");
    }

    prep_fs().context("Cannot prepare fs")?;

    // Replace stdio with one from /dev/ttyprintk
    let tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/ttyprintk")
        .context("Cannot open /dev/ttyprintk")?;
    dup2_stdin(tty.as_fd()).context("Cannot dup into stdin")?;
    dup2_stdout(tty.as_fd()).context("Cannot dup into stdout")?;
    dup2_stderr(tty.as_fd()).context("Cannot dup into stderr")?;
    drop(tty);

    let eventfd_for_service_mgr =
        EventFd::new().context("Cannot create service manager ready eventfd")?;
    let eventfd_for_sandboxer = unsafe {
        EventFd::from_owned_fd(
            dup(eventfd_for_service_mgr.as_fd())
                .context("Cannot dup service manager ready eventfd")?,
        )
    };

    let shutdown_req = EventFd::new().context("Cannot create shutdown req eventfd")?;
    let shutdown_acked = EventFd::new().context("Cannot create shutdown acked eventfd")?;

    let shutdown_req_for_service_mgr = unsafe {
        EventFd::from_owned_fd(
            dup(shutdown_req.as_fd()).context("Cannot dup shutdown req eventfd")?,
        )
    };

    let shutdown_acked_for_service_mgr = unsafe {
        EventFd::from_owned_fd(
            dup(shutdown_acked.as_fd()).context("Cannot dup shutdown acked eventfd")?,
        )
    };

    thread::spawn(move || {
        loop {
            match shutdown_req.read() {
                Ok(_) => break,
                Err(Errno::EINTR) => continue,
                Err(e) => unreachable!("Cannot read shutdown req: {e}"),
            }
        }

        INTENDING_TO_SHUTDOWN.store(true, Ordering::Relaxed);

        loop {
            match shutdown_acked.write(1) {
                Ok(_) => break,
                Err(Errno::EINTR) => continue,
                Err(e) => unreachable!("Cannot write shutdown ack: {e}"),
            }
        }
    });

    // We will wait outselves later, for why Child is dropped
    let server_pid = i32::try_from(
        Command::new("/system/bin/service_manager")
            .arg("--ready-event-fd")
            .arg(&format!("{}", eventfd_for_service_mgr.as_raw_fd()))
            .arg("--shutdown-req-to-init")
            .arg(&format!("{}", shutdown_req_for_service_mgr.as_raw_fd()))
            .arg("--shutdown-req-acked-from-init")
            .arg(&format!("{}", shutdown_acked_for_service_mgr.as_raw_fd()))
            .preserved_fds(vec![
                eventfd_for_service_mgr.into(),
                shutdown_req_for_service_mgr.into(),
                shutdown_acked_for_service_mgr.into(),
            ])
            .spawn()
            .context("Cannot spawn service_manager")?
            .id(),
    )
    .context("Service manager PID is too big")?;

    let sandboxer_pid = i32::try_from(
        Command::new("/system/bin/sandboxer")
            .arg("--service-manager-ready-event-fd")
            .arg(&format!("{}", eventfd_for_sandboxer.as_raw_fd()))
            .preserved_fds(vec![eventfd_for_sandboxer.into()])
            .spawn()
            .context("Cannot spawn service_manager")?
            .id(),
    )
    .context("Service manager PID is too big")?;

    thread::spawn(shell_spawner);

    fn check_shell(pid: Pid, is_abnormal_exit: bool) {
        // Check if its one of running root shells
        for shell in SHELLS.lock().iter_mut() {
            if let Some(active_pid) = shell.pid {
                if active_pid == pid {
                    println!("init: Debug shell {active_pid} is dead");
                    // Shell is dead
                    shell.pid = None;
                    shell.retry_at = Instant::now();
                    WAKE_SPAWNER.notify_one();

                    // Because it exits abnormally put timing delay
                    // to not spam. If it exits normally put 1 sec delay
                    if is_abnormal_exit {
                        shell.retry_at += ABNORMAL_RETRY_DURATION;
                    } else {
                        shell.retry_at += NORMAL_RETRY_DURATION;
                    }
                }
            }
        }
    }

    let mut service_manager_stopped = false;
    let mut sandboxer_stopped = false;

    while !service_manager_stopped || !sandboxer_stopped {
        match waitpid(None, None) {
            Ok(WaitStatus::Exited(pid, code)) => {
                println!("init: (repear) Process {pid} exited with {code}");
                if pid.as_raw() == server_pid {
                    if INTENDING_TO_SHUTDOWN.load(Ordering::Relaxed) {
                        if code == 0 {
                            service_manager_stopped = true;
                        } else {
                            bail!(
                                "Service manager failed during shutdown. It exited with code {code}"
                            );
                        }
                    } else {
                        bail!("Service manager must not exit at all. It exited with code {code}");
                    }
                }
                if pid.as_raw() == sandboxer_pid {
                    if INTENDING_TO_SHUTDOWN.load(Ordering::Relaxed) {
                        if code == 0 {
                            sandboxer_stopped = true;
                        } else {
                            bail!("Sandboxer failed during shutdown. It exited with code {code}");
                        }
                    } else {
                        bail!("Sandboxer must not exit at all. It exited with code {code}");
                    }
                }

                check_shell(pid, code != 0);
            }
            Ok(WaitStatus::Signaled(pid, signal, is_core_dumped)) => {
                println!(
                    "init: (repear) Process {pid} killed with {signal}{}",
                    if is_core_dumped { " (core dumped)" } else { "" }
                );

                if pid.as_raw() == sandboxer_pid {
                    bail!("Sandboxer must not exit at all. It killed by signal {signal}");
                } else if pid.as_raw() == server_pid {
                    bail!("Service manager must not exit at all. It killed by signal {signal}");
                }
                check_shell(pid, true);
            }
            Ok(WaitStatus::Stopped(_, _))
            | Err(Errno::EINTR)
            | Ok(WaitStatus::PtraceEvent(_, _, _))
            | Ok(WaitStatus::PtraceSyscall(_))
            | Ok(WaitStatus::Continued(_))
            | Ok(WaitStatus::StillAlive) => continue,
            Err(Errno::ECHILD) => {
                // Nothing lives anymore? do shutdown
                break;
            }
            x @ Err(_) => {
                x.context("Cannot do waitpid")?;
            }
        };
    }

    do_shutdown()
}

fn do_shutdown() -> anyhow::Result<Infallible> {
    println!("init: going to shutdown, bye bye! System server exited normally");

    kill(Pid::from_raw(-1), Signal::SIGKILL)
        .context("Cannot kill all processes when shutting down")?;

    // Now reap everything else
    loop {
        match waitpid(None, None) {
            Ok(WaitStatus::Stopped(_, _))
            | Err(Errno::EINTR)
            | Ok(WaitStatus::PtraceEvent(_, _, _))
            | Ok(WaitStatus::PtraceSyscall(_))
            | Ok(WaitStatus::Continued(_))
            | Ok(WaitStatus::StillAlive) => continue,
            Ok(WaitStatus::Exited(pid, code)) => {
                println!("init: shutdown: Process {pid} is exieted with {code}");
            }
            Ok(WaitStatus::Signaled(pid, signal, is_core_dumped)) => {
                if is_core_dumped {
                    println!(
                        "init: shutdown: Process {pid} is signalled and stopped with {signal} (core dumped)"
                    );
                } else {
                    println!(
                        "init: shutdown: Process {pid} is signalled and stopped with {signal}"
                    );
                }
            }

            // No more child
            Err(Errno::ECHILD) => break,
            x @ Err(_) => {
                x.context("Cannot waitpid")?;
            }
        }
    }

    sync();
    sleep(Duration::from_secs(1));
    Ok(reboot(RebootMode::RB_POWER_OFF).context("Cannot perform shutdown")?)
}

static SHELLS: LazyLock<Mutex<Box<[ShellState]>>> =
    LazyLock::new(|| Mutex::new(Box::new([ShellState::new("/dev/hvc1")])));
static WAKE_SPAWNER: Condvar = Condvar::new();
static ABNORMAL_RETRY_DURATION: Duration = Duration::from_secs(5);
static NORMAL_RETRY_DURATION: Duration = Duration::from_secs(1);

fn shell_spawner() {
    let mut shells = SHELLS.lock();
    loop {
        // Get earliest deadline so we can wakeup and retry spawning
        // if there shells failed
        let earlier_deadline = shells
            .iter()
            .map(|x| {
                if x.pid.is_some() {
                    None
                } else {
                    Some(x.retry_at)
                }
            })
            .flatten()
            .min();

        let condition = |x: &mut Box<[ShellState]>| {
            // Wait until there a shell that exit
            x.iter().find(|x| x.pid.is_none()).is_none()
        };
        if let Some(x) = earlier_deadline {
            WAKE_SPAWNER.wait_while_until(&mut shells, condition, x);
        } else {
            WAKE_SPAWNER.wait_while(&mut shells, condition);
        }

        for shell in shells
            .iter_mut()
            .filter(|x| x.pid.is_none())
            .filter(|x| Instant::now() >= x.retry_at)
        {
            // We retry shell that is just died or waited long enough
            match shell.spawn() {
                Ok(x) => {
                    println!("init: Respawned debug shell at {}", shell.dev);
                    shell.pid = Some(x);
                }
                Err(e) => {
                    println!(
                        "init: Cannot spawn shell at {}, retrying later: {e}",
                        shell.dev
                    );
                    shell.retry_at = Instant::now() + ABNORMAL_RETRY_DURATION;
                }
            }
        }
    }
}

struct ShellState {
    dev: &'static str,
    pid: Option<Pid>,
    retry_at: Instant,
}

impl ShellState {
    pub fn new(dev: &'static str) -> Self {
        Self {
            dev,
            pid: None,
            retry_at: Instant::now(),
        }
    }

    pub fn spawn(&mut self) -> anyhow::Result<Pid> {
        Ok(Pid::from_raw(
            Command::new("/system/bin/tty_spawner")
                .arg("--binary")
                .arg("/system/bin/busybox")
                .arg("--arg0")
                .arg("sh")
                .arg("--tty-dev")
                .arg(self.dev)
                .arg("--")
                .arg("-i")
                .spawn()?
                .id()
                .try_into()
                .context("Cannot convert u32 to i32 for PID")?,
        ))
    }
}

fn prep_fs() -> anyhow::Result<()> {
    let flags = MsFlags::MS_NOATIME
        | MsFlags::MS_NODIRATIME
        | MsFlags::MS_NODEV
        | MsFlags::MS_NOEXEC
        | MsFlags::MS_NOSUID;
    mount(
        None::<&Path>,
        "/tmp",
        Some("tmpfs"),
        flags,
        Some("size=32M"),
    )
    .context("Mounting init's /tmp")?;
    mount(
        None::<&Path>,
        "/mnt",
        Some("tmpfs"),
        flags,
        Some("size=32M"),
    )
    .context("Mounting /mnt tmpfs")?;

    // NOTE: devtmpfs is mounted by init, we dont need to do anything here

    mount(None::<&Path>, "/proc", Some("proc"), flags, None::<&Path>).context("Mounthing /proc")?;
    mount(None::<&Path>, "/sys", Some("sysfs"), flags, None::<&Path>).context("Mounthing /sys")?;
    mount(
        None::<&Path>,
        "/sys/fs/cgroup",
        Some("cgroup2"),
        flags,
        None::<&Path>,
    )
    .context("Mounthing /sys/fs/cgroup")?;

    mount(
        None::<&Path>,
        "/dev",
        Some("devtmpfs"),
        MsFlags::MS_NOATIME | MsFlags::MS_NODIRATIME | MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID,
        None::<&Path>,
    )
    .context("Mounthing /dev")?;

    fs::create_dir_all("/dev/binderfs").context("Creating binderfs")?;

    mount(
        None::<&Path>,
        "/dev/binderfs",
        Some("binder"),
        // The files under binderfs is inherently special device
        flags & !MsFlags::MS_NODEV,
        Some("stats=global"),
    )
    .context("Mounting /dev/binderfs")?;

    fchmodat(
        AT_FDCWD,
        "/dev/binderfs/binder",
        Mode::from_bits_retain(0o666),
        FchmodatFlags::NoFollowSymlink,
    )
    .context("Cannot chmod binder file")?;

    File::create_new("/dev/binder").context("Cannot create empty file at /dev/binder")?;

    mount(
        Some("/dev/binderfs/binder"),
        "/dev/binder",
        None::<&Path>,
        MsFlags::MS_BIND,
        None::<&Path>,
    )
    .context("Mount binding primary binder")?;
    Ok(())
}
