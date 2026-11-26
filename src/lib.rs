//! lens: structure-aware branch comparison for code review.
//!
//! Groups a diff into logical changes by linking changed symbols through references, and
//! reports unchanged code impacted by each change.

pub mod analysis;
pub mod diff;
pub mod facts;
pub mod git;
pub mod html;
pub mod index;
pub mod lang;
pub mod model;
pub mod render;
pub mod why;
