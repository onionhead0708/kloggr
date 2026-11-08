//! rlogg-core: the UI-independent engine of rlogg.
//!
//! Mirrors klogg's `src/logdata` + `src/regex` responsibilities:
//! - [`file`]: memory-mapped access to a log file (klogg: `FileHolder`)
//! - [`line_index`]: byte offsets of line starts (klogg: `LinePositionArray`,
//!   `CompressedLineStorage`)
//!
//! Planned: encoding detection, regex search / filtered data, file watching.

pub mod file;
pub mod line_index;

pub use file::LogFile;
pub use line_index::LineIndex;
