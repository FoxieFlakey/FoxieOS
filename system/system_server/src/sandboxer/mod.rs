use std::{
    collections::VecDeque,
    fs,
    os::fd::AsFd,
    path::Path,
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
    mount::{MsFlags, mount},
    sched::{CloneFlags, setns, unshare},
    sys::stat::Mode,
    unistd::{Uid, chroot, fchdir},
};
use openat::Dir;
use parking_lot::Mutex;

use crate::{constants::SANDBOXER_CGROUP_DIR, root_dev, sandboxer::sandbox::Sandbox};

pub mod sandbox;

pub struct Config<F>
where
    F: FnOnce(Uid) -> anyhow::Result<()>,
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
    F: FnOnce(Uid) -> anyhow::Result<()>,
{
    let orig_namespace = open(
        "/proc/self/ns/mnt",
        OFlag::O_CLOEXEC | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .context("Cannot save current namespace")?;
    let root_fd = open("/", OFlag::O_RDONLY | OFlag::O_CLOEXEC, Mode::empty())
        .context("Cannot save current root")?;

    // TODO: Cleaner way? doesnt harm because init, wont ever do chroot for entire process
    // so its fine each thread 'fs' is detached from the rest. Nor does init will chdir
    unshare(CloneFlags::CLONE_FS).context("Cannot unsahre CLONE_FS")?;

    // Create new namespace
    unshare(CloneFlags::CLONE_NEWNS).context("Cannot create new mount namespace")?;

    let unsandbox = |is_chrooted: bool| {
        if is_chrooted {
            fchdir(root_fd.as_fd()).expect("Cannot fchdir to old root");
            chroot(".").expect("Cannot restore root directory");
        }

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
    .inspect_err(|_| unsandbox(false))
    .context("Cannot save sandboxed namespace")?;

    mount(
        Some(root_dev::get_root_dev()),
        "/sandbox_root",
        Some("erofs"),
        MsFlags::MS_NODEV
            | MsFlags::MS_NOATIME
            | MsFlags::MS_NODIRATIME
            | MsFlags::MS_NOSUID
            | MsFlags::MS_RDONLY,
        None::<&Path>,
    )
    .inspect_err(|_| unsandbox(false))
    .context("Cannot mount sandbox root")?;

    let sandbox_root_fs = open(
        "/sandbox_root",
        OFlag::O_RDONLY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .inspect_err(|_| unsandbox(false))
    .context("Cannot open sandbox root directory")?;

    fchdir(sandbox_root_fs.as_fd()).context("Cannot fchdir to new root")?;
    chroot(".")
        .inspect_err(|_| {
            // chdir back to real root... if this fail. there would be
            // inconsistent state. We cannot restore
            // so panic
            fchdir(root_fd.as_fd()).expect("Cannot fchdir to old root");
        })
        .context("Cannot restore root directory")?;

    // Execute sandbox preparations
    (config.prep)(config.uid)
        .context("Cannot run sandbox preperation function")
        .inspect_err(|_| unsandbox(true))?;

    // Then restore back the original.
    unsandbox(true);

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
            root_fd: sandbox_root_fs,
        }),
    })
}
