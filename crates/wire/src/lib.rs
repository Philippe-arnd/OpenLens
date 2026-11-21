//! Project detection and install plans: finds a web app's stack and lists the
//! edits that plug OpenLens in.
//!
//! Used by `openlens-cli wire` and the MCP `wire_project` tool so an agent can
//! instrument an app (install the SDK, identify users with their role, name
//! features) with a reviewable plan instead of free-form edits.
//!
//! Layer: L2 (see `docs/architecture.md`). Status: not started (ROADMAP.md, M2).
