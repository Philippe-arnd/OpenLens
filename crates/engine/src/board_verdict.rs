//! The verdict of the newest release on the board (#614): the status
//! strip's *Latest release* cell reads it. It is judged as the Releases
//! page judges it ([`crate::verdict`]), by the server next to the board
//! and cached with it.

use serde::Serialize;

use crate::verdict::Report;

/// The newest release of the board's environment, judged.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LatestRelease {
    /// No release with traffic in the environment over the releases'
    /// window.
    NoRelease,
    /// The newest release compared with the one before it.
    Judged(Box<Report>),
    /// It could not be judged now: why.
    Unavailable {
        /// What failed.
        reason: String,
    },
}

impl LatestRelease {
    /// The report, when the release was judged.
    #[must_use]
    pub fn report(&self) -> Option<&Report> {
        match self {
            Self::Judged(r) => Some(r),
            _ => None,
        }
    }
}
