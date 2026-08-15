//! typst-mcp — render branded PDFs with Typst, over MCP and a REST API.
//!
//! The compiler is linked in-process rather than shelled out to, because
//! [`typst::World`] *is* the sandbox: implement `source()` and `file()` over an
//! in-memory bundle and there is no filesystem to escape and no network to reach.
//! See [`world`] for the containment and [`bundle`] for the path rules that feed it.

pub mod bundle;
pub mod compile;
pub mod diagnostics;
pub mod fonts;
pub mod world;
