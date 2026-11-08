//! rlogg-core: the UI-independent engine of rlogg.
//!
//! Mirrors klogg's `src/logdata` + `src/regex` responsibilities:
//! - [`file`]: memory-mapped access to a log file (klogg: `FileHolder`)
//! - [`encoding`]: encoding detection and decoding (klogg: `EncodingDetector`)
//! - [`line_index`]: parallel, incremental, compressed index of line starts
//!   (klogg: `LogDataWorker`, `CompressedLineStorage`)
//! - [`text`]: display text: tab expansion, long-line windowing (klogg: `untabify`)
//! - [`selection`]: line / range / portion selection (klogg: `Selection`)
//! - [`viewport`]: scroll position in lines and columns
//!
//! Planned: regex search / filtered data, file watching.

pub mod encoding;
pub mod file;
pub mod line_index;
pub mod selection;
pub mod text;
pub mod viewport;

pub use encoding::Encoding;
pub use file::{LogFile, OpenOptions};
pub use line_index::{Cancelled, IndexProgress, LineIndex};
pub use selection::{Drag, Position, Selection};
pub use viewport::Viewport;
