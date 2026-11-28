//! The plain half of fetching a file from the network, as each host does it: the names of a
//! download's files at every stage, when a file already here is trusted without being fetched again,
//! the words curl is given and the reading of what it says, how far a download is, the check of its
//! SHA-256, and every sentence. Each host's rules are a module of their own, as no two hosts fetch
//! the same way and a half download left by one version has to be read the same by the next.
//!
//! Only the Linux host fetches a file as it runs today, [`linux_host`]: Ubuntu's disc, the Android
//! image, remoteman's installer and the macOS part. The Windows service fetches nothing; the Windows
//! app's own update download keeps its stricter rules in `app-windows`, where its run of curl is.
//!
//! Running curl, reading a file's size and its SHA-256 from the disk, and writing the files stay with
//! each host.

pub mod linux_host;
