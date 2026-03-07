//! `pbuild` — a parallel, hash-based build engine.
//!
//! # Modules
//! - [`config`] — parses `pbuild.toml` into [`types::Rule`]s
//! - [`graph`] — topological sort of the dependency graph
//! - [`engine`] — parallel, wave-based rule execution with dirty-checking
//! - [`hash`] — SHA-256 file hashing and `.pbuild.lock` persistence
//! - [`process`] — subprocess runner
//! - [`types`] — core data types ([`types::Target`], [`types::Rule`])

pub mod config;
pub mod engine;
pub mod graph;
pub mod hash;
pub mod process;
pub mod types;
