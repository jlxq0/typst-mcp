//! typst-mcp — render branded PDFs with Typst, over MCP and a REST API.
//!
//! The compiler is linked in-process rather than shelled out to, because
//! [`typst::World`] *is* the sandbox: implement `source()` and `file()` over an
//! in-memory bundle and there is no filesystem to escape and no network to reach.
//! See [`world`] for the containment and [`bundle`] for the path rules that feed it.
//!
//! Compiles then run in a short-lived subprocess ([`spawn`], [`worker`]) — that is
//! what makes the deadline enforceable and what keeps Typst's process-global state
//! from accumulating. See [`worker`] for why that is not optional.

pub mod api;
pub mod auth;
pub mod bundle;
pub mod compile;
pub mod config;
pub mod diagnostics;
pub mod fonts;
pub mod mcp;
pub mod oidc;
pub mod principal;
pub mod protocol;
pub mod render;
pub mod server;
pub mod signing;
pub mod spawn;
pub mod store;
pub mod templates;
pub mod typst_value;
pub mod worker;
pub mod world;
