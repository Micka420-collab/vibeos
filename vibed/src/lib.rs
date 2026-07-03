//! vibed library crate: exposes the policy engine, audit log, glob matcher
//! and MCP server so that integration tests (`tests/`) can exercise them
//! against the real repository policy files. The daemon entry point lives
//! in `main.rs`.

pub mod audit;
pub mod glob;
pub mod mcp;
pub mod policy;
