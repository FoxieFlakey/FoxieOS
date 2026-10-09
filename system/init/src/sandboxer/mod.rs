use std::{
    collections::VecDeque,
    fs,
    os::fd::AsFd,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::Duration,
};

use anyhow::Context;
use nix::{
    fcntl::{OFlag, open},
    sched::{CloneFlags, setns, unshare},
    sys::stat::Mode,
    unistd::Uid,
};
use openat::Dir;
use parking_lot::Mutex;

use crate::{constants::SANDBOXER_CGROUP_DIR, sandboxer::sandbox::Sandbox};

pub mod sandbox;

pub struct Config<F>
where
    F: FnOnce() -> anyhow::Result<()>,
{
    // Function to be called to prep the sandbox
    // here its called when already in sandbox
    // but UID isn't changed. Run code to umount
    // mount, etc directories
    pub prep: F,

    // uid that all the tasks will spawn as
    pub uid: Uid,
}

static SANDBOX_DIR: OnceLock<Dir> = OnceLock::new();
static SANDBOX_ID_GENERATOR: AtomicU64 = AtomicU64::new(0);
static SANDBOX_ID_FREE: Mutex<VecDeque<u64>> = Mutex::new(VecDeque::new());

static SANDBOX_DESTROYING_QUEUE: OnceLock<mpsc::Sender<sandbox::Inner>> = OnceLock::new();

pub fn init() -> anyhow::Result<()> {
    fs::create_dir_all(SANDBOXER_CGROUP_DIR).context("Cannot create sandboxes cgroup")?;
    SANDBOX_DIR
        .set(Dir::open(SANDBOXER_CGROUP_DIR).context("Cannot open cgroup dir")?)
        .expect("Should not be initialized already");

    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let mut watched = Vec::new();
        loop {
            match receiver.recv_timeout(Duration::from_secs(5)) {
                Ok(x) => watched.push(x),
                Err(RecvTimeoutError::Timeout) => {
                    // Time to poll the watched
                    watched.retain_mut(|sandbox: &mut sandbox::Inner| match sandbox.destroy() {
                        Ok(()) => false,
                        Err(x) => {
                            println!(
                                "sandboxer: Sandbox {} cannot be destroyed, trying later: {x:#?}",
                                sandbox.id
                            );
                            true
                        }
                    });
                }
                Err(RecvTimeoutError::Disconnected) => panic!("Should not be disconnected"),
            }
        }
    });

    SANDBOX_DESTROYING_QUEUE
        .set(sender)
        .expect("Should not be initialized already");
    Ok(())
}

fn get_cgroup_dir() -> &'static Dir {
    SANDBOX_DIR.get().unwrap()
}

fn create_sandbox<F>(config: Config<F>) -> anyhow::Result<Sandbox>
where
    F: FnOnce() -> anyhow::Result<()>,
{
    let orig_namespace = open(
        "/proc/self/ns/mnt",
        OFlag::O_CLOEXEC | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .context("Cannot save current namespace")?;

    // Create new namespace
    unshare(CloneFlags::CLONE_NEWNS).context("Cannot create new mount namespace")?;

    let unsandbox = move || {
        // Panicking because now the caller thread
        // has inconsistent namespace compared to others. Which might creates unwanted
        // consequences
        setns(orig_namespace.as_fd(), CloneFlags::CLONE_NEWNS)
            .expect("Cannot restore current namespace");
    };

    let sandboxed_mnt_namespace = open(
        "/proc/self/ns/mnt",
        OFlag::O_CLOEXEC | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .inspect_err(|_| unsandbox())
    .context("Cannot save sandboxed namespace")?;

    // Execute sandbox preparations
    (config.prep)()
        .context("Cannot run sandbox preperation function")
        .inspect_err(|_| unsandbox())?;

    // Then restore back the original.
    unsandbox();

    let id = SANDBOX_ID_FREE
        .lock()
        .pop_front()
        .unwrap_or_else(|| SANDBOX_ID_GENERATOR.fetch_add(1, Ordering::Relaxed));

    get_cgroup_dir()
        .create_dir(&format!("{id}"), 0o600)
        .inspect_err(|_| SANDBOX_ID_FREE.lock().push_back(id))
        .context("Cannot create cgroup directory")?;
    let cgroup_dir = get_cgroup_dir()
        .sub_dir(&format!("{id}"))
        .inspect_err(|_| {
            if let Err(e) = get_cgroup_dir().remove_dir(&format!("{id}")) {
                println!("sandboxer: [warning] cannot remove sandbox cgroup dir for failed creation of {id}, leaking it: Cause: {e}");
            } else {
                SANDBOX_ID_FREE.lock().push_back(id)
            }
        })
        .context("Cannot open cgroup directory")?;

    Ok(Sandbox {
        inner: Some(sandbox::Inner {
            id,
            cgroup: cgroup_dir,
            uid: config.uid,
            mnt_namespace: sandboxed_mnt_namespace,
        }),
    })
}
