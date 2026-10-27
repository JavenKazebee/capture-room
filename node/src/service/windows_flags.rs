//! Process creation flags the service and `capture-room-launcher` start the
//! node with. Shared by path, since the launcher is its own crate root.

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
