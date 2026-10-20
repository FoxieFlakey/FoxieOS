use nix::unistd::Uid;

pub const INIT: Uid = Uid::from_raw(0);
pub const SERVICE_MANAER: Uid = Uid::from_raw(1001);

pub fn can_access_sandboxer(uid: Uid) -> bool {
    uid == SERVICE_MANAER
}
