use nix::unistd::Uid;

pub const INIT: Uid = Uid::from_raw(0);
pub const SANDBOXER: Uid = Uid::from_raw(1000);
pub const SERVICE_MANAER: Uid = Uid::from_raw(1001);

// This is not the "debug shell" from init. The debug shell from
// init is UID 0 but has no privilege to do actiosn with
// system services. While this shell has nonzero UID which checked
//
// mainly because sender_euid in binder transaction field is not
// reliable and defaults to 0 on some cases. So should be nonzero
// if checking prvilege by UID
pub const SHELL: Uid = Uid::from_raw(1002);

pub fn can_access_sandboxer(uid: Uid) -> bool {
    // Sandboxer has no business of creating/whatever own
    // sandboxes via binder IPC so no need to whitelist itself
    uid == SERVICE_MANAER
}
