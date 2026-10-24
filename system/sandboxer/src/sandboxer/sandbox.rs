use std::{
    io::Write,
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        linux::process::{ChildExt, CommandExt},
    },
    process::Command,
    sync::{Arc, Weak},
    thread::sleep,
    time::Duration,
};

use anyhow::{Context, anyhow};
use command_fds::CommandFdExt;
use foxie_base::{
    IObject, iobject,
    sandboxer::{IProcessHandle, ISandbox, SpawnArgs, isandbox},
    uids,
    util::retry_eintr,
};
use libbinder::object::{B, ObjectTrait, TransactionError};
use libbinder_basic::packetable::Serde;
use log::{debug, warn};
use nix::unistd::{Pid, Uid};
use openat::Dir;
use parking_lot::Mutex;
use pidfd_util::PidFdExtFoxie;

use crate::{
    constants::{SANDBOXER_HELPER_PATH, TTY_SPAWNER_PATH},
    process_handle::ProcessHandle,
    sandboxer::SANDBOX_ID_FREE,
};

pub(super) struct Inner {
    pub(super) is_destroyed: bool,
    pub(super) id: u64,
    pub(super) cgroup: Dir,
    pub(super) uid: Uid,
    pub(super) mnt_namespace: OwnedFd,
    pub(super) root_fd: OwnedFd,
}

pub struct Sandbox {
    pub(super) inner: Option<Inner>,
    pub(super) binder_runtime: Weak<libbinder::Runtime>,
}

impl Sandbox {
    fn get_inner(&self) -> &Inner {
        self.inner.as_ref().expect("Called on dead sandbox")
    }

    fn kill_all(&self) {
        let mut kill_file =
            retry_eintr(|| self.get_inner().cgroup.write_file("cgroup.kill", 0o600))
                .expect("Cannot open kill file");
        writeln!(&mut kill_file, "1").expect("Cannot write kill file");
    }

    fn spawn(&self, args: SpawnArgs) -> anyhow::Result<Arc<B<dyn IProcessHandle>>> {
        let procs_fd = retry_eintr(|| self.get_inner().cgroup.write_file("cgroup.procs", 0o600))
            .context("Cannot open cgroup.procs")
            .map(OwnedFd::from)?;
        let mnt_namespace_fd = nix::unistd::dup(self.get_inner().mnt_namespace.as_fd())
            .context("Cannot dup namespace fd")?;
        let root_fd = retry_eintr(|| nix::unistd::dup(self.get_inner().root_fd.as_fd()))
            .context("Cannot dup root fd")?;

        // Asking helper to downgrade privilege, mainly because its complicated
        // to do in same process a.k.a very fragile with .pre_exec
        let mut helper = if let Some(tty) = &args.tty {
            let mut cmd = Command::new(TTY_SPAWNER_PATH);
            cmd.arg("--tty-dev");
            cmd.arg(&**tty);
            cmd.arg("--binary");
            cmd.arg(SANDBOXER_HELPER_PATH);
            cmd.arg("--");
            cmd
        } else {
            Command::new(SANDBOXER_HELPER_PATH)
        };

        helper.arg("--procs-fd");
        helper.arg(format!("{}", procs_fd.as_raw_fd()));
        helper.arg("--mnt-namespace");
        helper.arg(format!("{}", mnt_namespace_fd.as_raw_fd()));
        helper.arg("--root-fd");
        helper.arg(format!("{}", root_fd.as_raw_fd()));
        helper.arg("--binary");
        helper.arg(&*args.path);
        helper.arg("--uid");
        helper.arg(format!("{}", self.get_inner().uid));
        helper.arg("--gid");
        helper.arg(format!("{}", self.get_inner().uid));
        if let Some(arg0) = &args.arg0 {
            helper.arg("--arg0");
            helper.arg(&**arg0);
        }
        helper.arg("--");

        for fd_arg in &args.fd_args {
            helper.arg(&fd_arg.0);
            helper.arg(format!("{}", fd_arg.1.as_raw_fd()));
        }
        helper.args(&args.argv);

        let mut preserved_fds = vec![procs_fd, mnt_namespace_fd, root_fd];
        preserved_fds.extend(args.fd_args.into_iter().map(|x| x.1));

        helper.preserved_fds(preserved_fds);
        helper.create_pidfd(true);

        let child = retry_eintr(|| helper.spawn())
            .context("Cannot spawn sandboxed process thru helper binary")?;
        let pid = child.id();
        let pidfd = child
            .into_pidfd()
            .ok()
            .expect("Already requested for pidfd...");

        let handle = Arc::new(B::new(ProcessHandle {
            pidfd: pidfd.dup().context("Cannot dup pidfd")?,
            death_reason: Mutex::new(None),
            death_listeners: Mutex::new(vec![]),
            pid: Pid::from_raw(pid as nix::libc::pid_t),
            rt: self.binder_runtime.clone(),
        }));

        super::REAPER_QUEUE
            .read()
            .as_ref()
            .unwrap()
            .send(handle.clone())
            .expect("Cannot send pidfd to sandboxer's reaper to be watched");
        retry_eintr(|| super::REAPER_WAKE.write(1)).expect("Cannot wake up sandboxer's reaper");

        Ok(handle)
    }
}

impl Inner {
    // This function must be idempotent
    // it should not try destroy twice
    pub(crate) fn destroy(&mut self) -> anyhow::Result<()> {
        if self.is_destroyed {
            return Ok(());
        }

        // Try rm dir the cgroup directory
        retry_eintr(|| super::get_cgroup_dir().remove_dir(&format!("{}", self.id)))
            .context("Removing cgroup dir")?;

        // Succesfully destroyed, release the ID
        debug!("sandboxer: Destroyed sandbox: {}", self.id);
        SANDBOX_ID_FREE.lock().push_back(self.id);

        self.is_destroyed = true;
        Ok(())
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Kill all processes inside. Because this is &mut
        // there no way that new process can spawn inside
        // from outside. From inside it is all death
        self.kill_all();

        let mut sandbox = self.inner.take().unwrap();
        if let Err(_) = sandbox.destroy() {
            // At the moment it cant be free'd immediately
            // queue it for later
            match super::SANDBOX_DESTROYING_QUEUE.read().as_ref() {
                Some(queue) => {
                    queue.send(sandbox).unwrap();
                }
                None => {
                    // The background thread gone, so lets block here
                    // and retries 5 times
                    let mut err = None;
                    for _ in 0..5 {
                        sleep(Duration::from_secs(1));

                        if let Err(e) = sandbox.destroy() {
                            err = Some(e);
                        }
                    }

                    if let Some(e) = err {
                        warn!(
                            "sandboxer: We're shutting down but sandbox {} cannot be destroyed (ignoring after 5 attemps): {e:#}",
                            sandbox.id
                        );
                    }
                }
            };
        }
    }
}

// IPC details

impl ObjectTrait for Sandbox {
    fn get_remote<'a>(&'a self) -> Option<&'a libbinder::proxy::Proxy> {
        None
    }

    fn get_runtime(&self) -> std::sync::Arc<libbinder::Runtime> {
        self.binder_runtime.upgrade().unwrap()
    }

    fn on_transaction(
        &self,
        code: u32,
        flags: libbinder_basic::enumflags2::BitFlags<libbinder::object::Flag>,
        message: &mut libbinder::packet::Packet,
    ) -> Result<Option<(u32, libbinder::packet::Packet)>, libbinder::object::TransactionError> {
        <dyn ISandbox>::decode_and_dispatch(self, code, flags, message)
    }
}

impl IObject for Sandbox {
    fn has_interface(&self, name: &str) -> Result<bool, TransactionError> {
        match name {
            iobject::ID => Ok(true),
            isandbox::ID => Ok(true),
            _ => Ok(false),
        }
    }
}

impl ISandbox for Sandbox {
    fn kill_all(&self) -> Result<anyhow::Result<Serde<()>>, TransactionError> {
        if !uids::can_access_sandboxer(self.get_runtime().get_caller_identity().sender_euid) {
            return Ok(Err(anyhow!("Caller not allowed to access sandboxer")));
        }
        Ok(Ok(Serde(self.kill_all())))
    }

    fn spawn(
        &self,
        spawn_args: SpawnArgs,
    ) -> Result<anyhow::Result<Arc<B<dyn IProcessHandle>>>, TransactionError> {
        if !uids::can_access_sandboxer(self.get_runtime().get_caller_identity().sender_euid) {
            return Ok(Err(anyhow!("Caller not allowed to access sandboxer")));
        }
        Ok(self.spawn(spawn_args))
    }
}
