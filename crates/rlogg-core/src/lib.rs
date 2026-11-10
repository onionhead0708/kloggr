//! rlogg-core: the UI-independent engine of rlogg.
//!
//! Mirrors klogg's `src/logdata` + `src/regex` responsibilities:
//! - [`file`]: memory-mapped access to a log file (klogg: `FileHolder`)
//! - [`encoding`]: encoding detection and decoding (klogg: `EncodingDetector`)
//! - [`line_index`]: parallel, incremental, compressed index of line starts
//!   (klogg: `LogDataWorker`, `CompressedLineStorage`)
//! - [`text`]: display text: tab expansion, long-line windowing (klogg: `untabify`)
//! - [`digits`]: the number typed before a view command (klogg: `DigitsBuffer`)
//! - [`selection`]: line / range / portion selection (klogg: `Selection`)
//! - [`viewport`]: scroll position in lines and columns
//! - [`settings`]: persistent session state, recent files (klogg: `RecentFiles`)
//! - [`searches`]: search history and predefined filters (klogg: `SavedSearches`,
//!   `PredefinedFiltersCollection`)
//! - [`options`]: user options and shortcut overrides (klogg: `Configuration`)
//! - [`search`]: patterns, matchers and parallel search (klogg: `src/regex`,
//!   `LogFilteredDataWorker`)
//!
//! - [`watch`]: file change notifications (klogg: `filewatch/`)
//! - [`highlight`]: highlighters, highlighter sets and color labels (klogg:
//!   `HighlighterSet`, `ColorLabelsManager`)

pub mod digits;
pub mod encoding;
pub mod file;
pub mod highlight;
pub mod line_index;
pub mod options;
pub mod search;
pub mod searches;
pub mod selection;
pub mod settings;
pub mod text;
pub mod viewport;
pub mod watch;

pub use digits::DigitsBuffer;
pub use encoding::Encoding;
pub use file::{FileChange, LogFile, OpenOptions};
pub use line_index::{Cancelled, IndexProgress, LineIndex};
pub use search::{LineSet, Matcher, Pattern, PatternError, SearchProgress};
pub use selection::{Drag, Position, Selection};
pub use viewport::Viewport;
pub use watch::Watcher;
