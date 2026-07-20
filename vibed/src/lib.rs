//! vibed library crate: exposes the policy engine, audit log, glob matcher
//! and MCP server so that integration tests (`tests/`) can exercise them
//! against the real repository policy files. The daemon entry point lives
//! in `main.rs`.

pub mod approval;
pub mod audit;
pub mod cdp;
pub mod domain;
pub mod glob;
pub mod mcp;
pub mod policy;
pub mod ratelimit;
pub mod reasoning;
pub mod sandbox;
pub mod sha256;
pub mod supervisor;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tools;
pub mod vibectl;
