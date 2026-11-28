//! Open flags and attribute masks, with the values 9P2000.L takes from Linux on x86 and Arm.

/// Open flags, for `lopen` and `lcreate`. These are Linux's numbers, not Windows'.
pub mod open {
    pub const O_RDONLY: u32 = 0;
    pub const O_WRONLY: u32 = 1;
    pub const O_RDWR: u32 = 2;
    /// The bits that say how the file is opened for access.
    pub const O_ACCMODE: u32 = 3;
    pub const O_CREAT: u32 = 0o100;
    pub const O_EXCL: u32 = 0o200;
    pub const O_NOCTTY: u32 = 0o400;
    pub const O_TRUNC: u32 = 0o1000;
    pub const O_APPEND: u32 = 0o2000;
    pub const O_NONBLOCK: u32 = 0o4000;
    pub const O_DSYNC: u32 = 0o10000;
    pub const O_DIRECTORY: u32 = 0o200000;
    pub const O_NOFOLLOW: u32 = 0o400000;
    pub const O_SYNC: u32 = 0o4010000;
}

/// What `getattr` is asked for, and what it says it filled in.
pub mod getattr {
    pub const MODE: u64 = 0x1;
    pub const NLINK: u64 = 0x2;
    pub const UID: u64 = 0x4;
    pub const GID: u64 = 0x8;
    pub const RDEV: u64 = 0x10;
    pub const ATIME: u64 = 0x20;
    pub const MTIME: u64 = 0x40;
    pub const CTIME: u64 = 0x80;
    pub const INO: u64 = 0x100;
    pub const SIZE: u64 = 0x200;
    pub const BLOCKS: u64 = 0x400;
    /// Everything `stat` has.
    pub const BASIC: u64 = 0x7ff;
    pub const BTIME: u64 = 0x800;
    pub const GEN: u64 = 0x1000;
    pub const DATA_VERSION: u64 = 0x2000;
    pub const ALL: u64 = 0x3fff;
}

/// Which fields of a `setattr` count.
pub mod setattr {
    pub const MODE: u32 = 0x1;
    pub const UID: u32 = 0x2;
    pub const GID: u32 = 0x4;
    pub const SIZE: u32 = 0x8;
    /// Set the access time to now, unless `ATIME_SET` says to use the one sent.
    pub const ATIME: u32 = 0x10;
    pub const MTIME: u32 = 0x20;
    pub const CTIME: u32 = 0x40;
    pub const ATIME_SET: u32 = 0x80;
    pub const MTIME_SET: u32 = 0x100;
}

/// `unlinkat`'s flag for removing a directory.
pub const AT_REMOVEDIR: u32 = 0x200;

/// The mode bits that say what a file is.
pub mod mode {
    pub const S_IFMT: u32 = 0o170000;
    pub const S_IFDIR: u32 = 0o040000;
    pub const S_IFREG: u32 = 0o100000;
    pub const S_IFLNK: u32 = 0o120000;
}

/// The directory entry types `readdir` sends, which are `DT_*` from Linux.
pub mod dt {
    pub const DIR: u8 = 4;
    pub const REG: u8 = 8;
    pub const LNK: u8 = 10;
}
