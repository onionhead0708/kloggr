//! rlogg-core: the UI-independent engine of rlogg.
//!
//! Mirrors klogg's `src/logdata` + `src/regex` responsibilities:
//! - [`file`]: memory-mapped access to a log file (klogg: `FileHolder`)
//! - [`encoding`]: encoding detection and decoding (klogg: `EncodingDetector`)
//! - [`line_index`]: parallel, incremental, compressed index of line starts
//!   (klogg: `LogDataWorker`, `CompressedLineStorage`)
//!
//! Planned: regex search / filtered data, file watching.

pub mod encoding;
pub mod file;
pub mod line_index;

pub use encoding::Encoding;
pub use file::{LogFile, OpenOptions};
pub use line_index::{Cancelled, IndexProgress, LineIndex};
