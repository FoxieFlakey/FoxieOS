use std::{
    process::Command,
    sync::LazyLock,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use nix::{
    sys::wait::{WaitStatus, waitpid},
    unistd::Pid,
};
use parking_lot::{Condvar, Mutex};

fn main() -> anyhow::Result<()> {
    if Pid::this().as_raw() != 1 {
        bail!("Must run as PID 1");
    }

    // We will wait outselves later, for why Child is dropped
    let server_pid = i32::try_from(
        Command::new("/system/bin/system_server")
            .spawn()
            .context("Cannot spawn system_server")?
            .id(),
    )
    .context("System server PID is too big")?;

    thread::spawn(shell_spawner);

    loop {
        match waitpid(None, None) {
            Ok(WaitStatus::Exited(pid, code)) => {
                println!("init: (repear) Process {pid} exited with {code}");
                if pid.as_raw() == server_pid {
                    bail!("System server must not exit at all. It exited with code {code}");
                }

                // Check if its one of running root shells
                for shell in SHELLS.lock().iter_mut() {
                    if let Some(active_pid) = shell.pid {
                        if active_pid == pid {
                            // Shell is dead
                            shell.pid = None;
                            shell.retry_at = Instant::now();
                            WAKE_SPAWNER.notify_one();

                            // Because it exits abnormally put timing delay
                            // to not spam. If it exits normally put 1 sec delay
                            if code != 0 {
                                shell.retry_at += NORMAL_RETRY_DURATION;
                            } else {
                                shell.retry_at += ABNORMAL_RETRY_DURATION;
                            }
                        }
                    }
                }
            }
            Ok(_) | Err(_) => {
                nix::unistd::sleep(5);
            }
        };
    }
}

static SHELLS: LazyLock<Mutex<Box<[ShellState]>>> =
    LazyLock::new(|| Mutex::new(Box::new([ShellState::new("/dev/hvc0")])));
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
            x.iter().find(|x| x.pid.is_none()).is_some()
        };
        if let Some(x) = earlier_deadline {
            WAKE_SPAWNER.wait_while_until(&mut shells, condition, x);
        } else {
            WAKE_SPAWNER.wait_while(&mut shells, condition);
        }

        for shell in shells
            .iter_mut()
            .filter(|x| x.pid.is_none())
            .filter(|x| Instant::now() > x.retry_at)
        {
            // We retry shell that is just died or waited long enough
            match shell.spawn() {
                Ok(x) => {
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
            // Lets hardcode so it retries 2 secs later
            // because its constructed very early in init
            // lifetime, the system_server might not started
            // yet to mount /dev so give small delay to
            // allow it do that
            retry_at: Instant::now() + Duration::from_secs(2),
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
