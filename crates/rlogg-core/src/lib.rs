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
//! - [`settings`]: persistent session state, recent files (klogg: `RecentFiles`)
//! - [`search`]: patterns, matchers and parallel search (klogg: `src/regex`,
//!   `LogFilteredDataWorker`)
//!
//! Planned: filtered data (results + marks), file watching.

pub mod encoding;
pub mod file;
pub mod line_index;
pub mod search;
pub mod selection;
pub mod settings;
pub mod text;
pub mod viewport;

pub use encoding::Encoding;
pub use file::{LogFile, OpenOptions};
pub use line_index::{Cancelled, IndexProgress, LineIndex};
pub use search::{LineSet, Matcher, Pattern, PatternError, SearchProgress};
pub use selection::{Drag, Position, Selection};
pub use viewport::Viewport;
