use std::{
    collections::{HashMap, VecDeque},
    ffi::CString,
    fs,
    os::fd::AsFd,
    path::Path,
    sync::{
        Arc, LazyLock, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::Context;
use foxie_base::{common::SerdeSignal, sandboxer::ExitReason, util::retry_eintr};
use libbinder::object::B;
use nix::{
    fcntl::{OFlag, open},
    mount::{MntFlags, MsFlags, mount, umount2},
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sched::{CloneFlags, setns, unshare},
    sys::{
        epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags},
        eventfd::{EfdFlags, EventFd},
        prctl,
        stat::Mode,
    },
    unistd::{Uid, chroot, fchdir},
};
use openat::Dir;
use parking_lot::{Mutex, RwLock};
use pidfd_util::PidFdExtFoxie;

use crate::{
    constants::SANDBOXER_CGROUP_DIR, process_handle::ProcessHandle, root_dev,
    sandboxer::sandbox::Sandbox,
};

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

static SANDBOX_DESTROYING_QUEUE: RwLock<Option<mpsc::Sender<sandbox::Inner>>> = RwLock::new(None);

static BACKGROUND_THRD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

static REAPER_THRD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
static REAPER_QUEUE: RwLock<Option<mpsc::Sender<Arc<B<ProcessHandle>>>>> = RwLock::new(None);
static REAPER_WAKE: LazyLock<EventFd> =
    LazyLock::new(|| EventFd::from_value_and_flags(0, EfdFlags::EFD_CLOEXEC).unwrap());

pub fn shutdown() {
    drop(SANDBOX_DESTROYING_QUEUE.write().take());
    BACKGROUND_THRD.lock().take().unwrap().join().unwrap();

    drop(REAPER_QUEUE.write().take());
    retry_eintr(|| REAPER_WAKE.write(1)).expect("Cannot wake reaper");
    REAPER_THRD.lock().take().unwrap().join().unwrap();
}

pub fn init() -> anyhow::Result<()> {
    fs::create_dir_all(SANDBOXER_CGROUP_DIR).context("Cannot create sandboxes cgroup")?;
    SANDBOX_DIR
        .set(Dir::open(SANDBOXER_CGROUP_DIR).context("Cannot open cgroup dir")?)
        .expect("Should not be initialized already");

    let (sender, receiver) = mpsc::channel::<Arc<B<ProcessHandle>>>();
    let reaper = thread::spawn(move || {
        prctl::set_name(&CString::new("Reaper").unwrap()).expect("Cannot rename reapear thread");

        let mut watched: HashMap<u64, Arc<B<ProcessHandle>>> = HashMap::new();
        let mut id = 0;

        let epoll = Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC).expect("Cannot create epoll");
        'looper: loop {
            let mut pollfds = [
                PollFd::new(epoll.0.as_fd(), PollFlags::POLLIN),
                PollFd::new(REAPER_WAKE.as_fd(), PollFlags::POLLIN),
            ];

            retry_eintr(|| poll(&mut pollfds, PollTimeout::NONE)).expect("Cannot poll");

            if !pollfds[0].revents().unwrap().is_empty() {
                let mut events = [EpollEvent::empty(); 100];
                let count = retry_eintr(|| epoll.wait(&mut events, None::<PollTimeout>))
                    .expect("Cannot epoll on pifds");

                for event in &events[0..count] {
                    // Do reap
                    let process = watched
                        .remove(&event.data())
                        .expect("There should be pidfd here");
                    retry_eintr(|| epoll.delete(process.pidfd.as_fd()))
                        .expect("Cannot remove pidfd from epoll");

                    retry_eintr(|| process.pidfd.wait()).expect("Cannot reap process");
                    let mut death_reason = process.death_reason.lock();
                    match process
                        .pidfd
                        .get_termination_reason()
                        .expect("Cannot get termination reason")
                    {
                        nix::sys::wait::WaitStatus::Exited(_, code) => {
                            *death_reason = Some(ExitReason::Exit(code));
                        }
                        nix::sys::wait::WaitStatus::Signaled(_, signal, _) => {
                            *death_reason = Some(ExitReason::Signal(SerdeSignal(signal)));
                        }
                        _ => unreachable!(),
                    }

                    // Wake up listeners
                    for listener in process.death_listeners.lock().drain(..) {
                        // Best effort only
                        let _ = listener.call();
                    }

                    drop(death_reason);
                }
            }

            // There request incoming
            if !pollfds[1].revents().unwrap().is_empty() {
                retry_eintr(|| REAPER_WAKE.read()).expect("Cannot ackowledge the wakeup");
                loop {
                    match receiver.try_recv() {
                        Ok(x) => {
                            epoll
                                .add(x.pidfd.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, id))
                                .expect("Cannot add new pidfd to be watched");

                            // New PID to watch for
                            watched.insert(id, x);
                            id += 1;
                        }
                        Err(TryRecvError::Disconnected) => {
                            // There is shutdown requested
                            break 'looper;
                        }
                        Err(TryRecvError::Empty) => {
                            break;
                        }
                    }
                }
            }
        }
    });
    *REAPER_THRD.lock() = Some(reaper);
    *REAPER_QUEUE.write() = Some(sender);

    let (sender, receiver) = mpsc::channel();

    let background = thread::spawn(move || {
        prctl::set_name(&CString::new("Sandbox cleaner").unwrap())
            .expect("Cannot rename sandbox cleaner thread");
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
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    *BACKGROUND_THRD.lock() = Some(background);
    *SANDBOX_DESTROYING_QUEUE.write() = Some(sender);
    Ok(())
}

fn get_cgroup_dir() -> &'static Dir {
    SANDBOX_DIR.get().unwrap()
}

pub fn create_sandbox<F>(
    config: Config<F>,
    binder_runtime: Weak<libbinder::Runtime>,
) -> anyhow::Result<Sandbox>
where
    F: FnOnce(Uid) -> anyhow::Result<()>,
{
    let orig_namespace = open(
        "/proc/thread-self/ns/mnt",
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
        "/proc/thread-self/ns/mnt",
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

    mount(
        Some("/"),
        "/sandbox_root/orig_root",
        None::<&Path>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&Path>,
    )
    .inspect_err(|_| unsandbox(false))
    .context("Cannot mount bind original root")?;

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

    // Unmount original root, to prevent it leaking out
    umount2(
        "/orig_root",
        MntFlags::UMOUNT_NOFOLLOW | MntFlags::MNT_DETACH,
    )
    .context("Cannot unmount original root")
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
            is_destroyed: false,
            cgroup: cgroup_dir,
            uid: config.uid,
            mnt_namespace: sandboxed_mnt_namespace,
            root_fd: sandbox_root_fs,
        }),
        binder_runtime,
    })
}
