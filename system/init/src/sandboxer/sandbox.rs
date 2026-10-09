use std::{
    io::Write,
    os::fd::{AsFd, AsRawFd, OwnedFd},
    process::{Child, Command},
};

use anyhow::Context;
use command_fds::CommandFdExt;
use nix::unistd::Uid;
use openat::Dir;

use crate::{constants::SANDBOXER_HELPER_PATH, sandboxer::SANDBOX_ID_FREE};

pub(super) struct Inner {
    pub(super) id: u64,
    pub(super) cgroup: Dir,
    pub(super) uid: Uid,
    pub(super) mnt_namespace: OwnedFd,
}

pub struct Sandbox {
    pub(super) inner: Option<Inner>,
}

pub struct SpawnArgs {
    pub path: String,
    pub argv: Vec<String>,
    pub arg0: Option<String>,
}

impl Sandbox {
    pub fn new<F>(config: super::Config<F>) -> anyhow::Result<Self>
    where
        F: FnOnce() -> anyhow::Result<()>,
    {
        super::create_sandbox(config)
    }

    fn get_inner(&self) -> &Inner {
        self.inner.as_ref().expect("Called on dead sandbox")
    }

    pub fn kill_all(&self) {
        let mut kill_file = self
            .get_inner()
            .cgroup
            .write_file("cgroup.kill", 0o600)
            .expect("Cannot open kill file");
        writeln!(&mut kill_file, "1").expect("Cannot write kill file");
    }

    pub fn spawn(&self, args: &SpawnArgs) -> anyhow::Result<Child> {
        let procs_fd = self
            .get_inner()
            .cgroup
            .write_file("cgroup.procs", 0o600)
            .context("Cannot open cgroup.procs")
            .map(OwnedFd::from)?;
        let mnt_namespace_fd = nix::unistd::dup(self.get_inner().mnt_namespace.as_fd())
            .context("Cannot dup namespace fd")?;

        // Asking helper to downgrade privilege, mainly because its complicated
        // to do in same process a.k.a very fragile with .pre_exec
        let mut helper = Command::new(SANDBOXER_HELPER_PATH);

        helper.arg("--procs-fd");
        helper.arg(format!("{}", procs_fd.as_raw_fd()));
        helper.arg("--mnt-namespace");
        helper.arg(format!("{}", mnt_namespace_fd.as_raw_fd()));
        helper.arg("--binary");
        helper.arg(&args.path);
        helper.arg("--uid");
        helper.arg(format!("{}", self.get_inner().uid));
        helper.arg("--gid");
        helper.arg(format!("{}", self.get_inner().uid));
        if let Some(arg0) = &args.arg0 {
            helper.arg("--arg0");
            helper.arg(arg0);
        }
        helper.arg("--");
        helper.args(&args.argv);

        helper.preserved_fds(vec![procs_fd, mnt_namespace_fd]);

        Ok(helper
            .spawn()
            .context("Cannot spawn sandboxed process thru helper binary")?)
    }
}

impl Inner {
    // This function must be idempotent
    // it should not try destroy twice
    pub(crate) fn destroy(&mut self) -> anyhow::Result<()> {
        // Try rm dir the cgroup directory
        super::get_cgroup_dir()
            .remove_dir(&format!("{}", self.id))
            .context("Removing cgroup dir")?;

        // Succesfully destroyed, release the ID
        println!("sandboxer: Destroyed sandbox: {}", self.id);
        SANDBOX_ID_FREE.lock().push_back(self.id);
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
            super::SANDBOX_DESTROYING_QUEUE
                .get()
                .unwrap()
                .send(sandbox)
                .unwrap();
        }
    }
}
