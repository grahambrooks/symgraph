//! CLI command implementations
//!
//! Handles all command-line interface operations:
//! - index: Index a codebase
//! - status: Show index statistics  
//! - search: Search for symbols
//! - context: Build AI context for tasks

mod commands;
mod db_utils;
pub mod tools;

pub use commands::*;
pub use db_utils::*;
