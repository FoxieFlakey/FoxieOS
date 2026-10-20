use nix::unistd::Uid;

pub const INIT: Uid = Uid::from_raw(0);
pub const SERVICE_MANAER: Uid = Uid::from_raw(1001);
