//! Operating-system pieces of the LAN link. unix.rs (macOS and Linux) and
//! windows.rs provide the same items, and the rest of the crate never branches
//! on the operating system:
//!   wake_pair, Wake, Waiter, wait, Wait   listener cancellation
//!   addresses                             this device's LAN IPv4 addresses
//!   create_directory, protect_directory,  owner-only credentials on disk
//!     private_file
#[cfg(unix)]
#[path = "unix.rs"]
mod host;
#[cfg(windows)]
#[path = "windows.rs"]
mod host;
pub(crate) use host::*;
