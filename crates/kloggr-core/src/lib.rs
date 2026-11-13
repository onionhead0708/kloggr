//! kloggr-core: the UI-independent engine of kloggr.
//!
//! Mirrors klogg's `src/logdata` + `src/regex` responsibilities:
//! - [`archive`]: compressed files and archives (klogg: `Decompressor`)
//! - [`file`]: memory-mapped access to a log file (klogg: `FileHolder`)
//! - [`encoding`]: encoding detection and decoding (klogg: `EncodingDetector`)
//! - [`line_index`]: parallel, incremental, compressed index of line starts
//!   (klogg: `LogDataWorker`, `CompressedLineStorage`)
//! - [`text`]: display text: tab expansion, long-line windowing (klogg: `untabify`)
//! - [`download`]: downloading a file from a URL (klogg: `Downloader`)
//! - [`digits`]: the number typed before a view command (klogg: `DigitsBuffer`)
//! - [`selection`]: line / range / portion selection (klogg: `Selection`)
//! - [`viewport`]: scroll position in lines and columns
//! - [`settings`]: persistent session state, recent files (klogg: `RecentFiles`)
//! - [`scratch`]: the scratchpad's transformations (klogg: `ScratchPad`)
//! - [`searches`]: search history and predefined filters (klogg: `SavedSearches`,
//!   `PredefinedFiltersCollection`)
//! - [`options`]: user options and shortcut overrides (klogg: `Configuration`)
//! - [`search`]: patterns, matchers and parallel search (klogg: `src/regex`,
//!   `LogFilteredDataWorker`)
//!
//! - [`instance`]: one kloggr at a time (klogg: `SingleApplication`)
//! - [`watch`]: file change notifications (klogg: `filewatch/`)
//! - [`highlight`]: highlighters, highlighter sets and color labels (klogg:
//!   `HighlighterSet`, `ColorLabelsManager`)

/// Where kloggr is developed and released.
pub const REPOSITORY: &str = "https://github.com/onionhead0708/kloggr";

pub mod archive;
pub mod crash;
pub mod digits;
pub mod download;
pub mod encoding;
pub mod file;
pub mod highlight;
pub mod i18n;
pub mod instance;
pub mod line_index;
pub mod options;
mod qt_ini;
pub mod scratch;
pub mod search;
pub mod searches;
pub mod selection;
pub mod settings;
pub mod text;
pub mod version_check;
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
