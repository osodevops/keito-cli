//! Unix process ownership shared by product and developer tools.
//!
//! Group/protocol extraction only: product call sites, short deadlines,
//! cooperative core and bounded I/O integration are still pending. Merely
//! compiling this module in a default artifact is not runtime qualification.
//! This code grants no capture, profile, billing or authentication authority.
pub mod group;
pub mod protocol;
