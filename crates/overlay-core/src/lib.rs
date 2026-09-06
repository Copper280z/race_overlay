//! Stable, renderer-independent data model and import utilities for Race Overlay.
//!
//! All timestamps are seconds.  A source offset is deliberately defined as
//! `source_time = video_time + offset_seconds`.

pub mod adapters;
pub mod analysis;
pub mod calibration;
pub mod processing;
pub mod project;
pub mod telemetry;
mod xrk;

pub use adapters::*;
pub use analysis::*;
pub use calibration::*;
pub use processing::*;
pub use project::*;
pub use telemetry::*;
