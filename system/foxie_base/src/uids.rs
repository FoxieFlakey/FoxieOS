use nix::unistd::Uid;

pub const INIT: Uid = Uid::from_raw(0);
pub const SANDBOXER: Uid = Uid::from_raw(1000);
pub const SERVICE_MANAER: Uid = Uid::from_raw(1001);

pub fn can_access_sandboxer(uid: Uid) -> bool {
    // Sandboxer has no business of creating/whatever own
    // sandboxes via binder IPC so no need to whitelist itself
    uid == SERVICE_MANAER
}
