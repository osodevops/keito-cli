//! Unix process ownership shared by product and developer tools.
//!
//! Shared group/protocol code includes closed legacy and short cleanup policies.
//! Product call sites, cooperative core and bounded I/O wiring remain pending. Merely
//! compiling this module in a default artifact is not runtime qualification.
//! This code grants no capture, profile, billing or authentication authority.
mod attachment;
pub mod group;
pub(crate) mod image;
pub mod io;
pub(crate) mod probes;
pub mod protocol;
pub mod scope;
pub(crate) mod signals;
