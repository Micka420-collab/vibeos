//! Tool implementations, split out of `mcp.rs` by family (F6 — mechanical
//! refactor, zero behaviour change). The MCP wiring (connection handling, the
//! tool catalog, the dispatch in `execute_tool`, the audit/approval flow) stays
//! in `mcp.rs`; each `tools::<family>` module holds the pure implementations for
//! one family, with its own tests.

pub(crate) mod fs;
pub(crate) mod memory;
pub(crate) mod sectools;
pub(crate) mod svc;
