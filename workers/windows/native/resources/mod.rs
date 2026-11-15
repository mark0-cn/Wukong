//! Authoritative resource lifecycle, reservations and recovery.
pub(crate) mod journal;
pub(crate) mod manager;
mod observation;
pub(crate) mod operation;
mod recovery_observer;
pub mod resource_recovery;
pub(crate) mod state;
