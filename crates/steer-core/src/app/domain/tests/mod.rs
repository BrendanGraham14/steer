//! Domain tests module.
//!
//! This module contains comprehensive tests for the domain layer:
//! - Golden tests: Snapshot-based tests for major flows
//! - Property tests: Proptest-based randomized testing for invariants
//! - Replay tests: Event sourcing consistency tests

mod golden;
mod property;
mod replay;
