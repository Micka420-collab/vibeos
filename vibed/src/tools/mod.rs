//! Tool implementations, split out of `mcp.rs` by family (F6 — mechanical
//! refactor, zero behaviour change). The MCP wiring (connection handling, the
//! tool catalog, the dispatch in `execute_tool`, the audit/approval flow) stays
//! in `mcp.rs`; each `tools::<family>` module holds the pure implementations for
//! one family, with its own tests.

pub(crate) mod browser;
pub(crate) mod consolidate;
pub(crate) mod deploy;
pub(crate) mod embed;
pub(crate) mod embeddings;
pub(crate) mod fs;
pub(crate) mod identity;
pub(crate) mod log;
pub(crate) mod memory;
pub(crate) mod policy_tool;
pub(crate) mod propose;
pub(crate) mod recall;
pub(crate) mod sandbox_tool;
pub(crate) mod sectools;
pub(crate) mod svc;
pub(crate) mod user_model;
