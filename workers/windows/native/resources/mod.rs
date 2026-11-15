//! Authoritative resource lifecycle, reservations and recovery.
pub(crate) mod journal;
pub(crate) mod manager;
pub mod models;
mod observation;
mod recovery_observer;
pub mod resource_recovery;
pub(crate) mod state;
