//! Scanner: FS watch (notify), filename parsing, ffprobe ingest, incremental rescans.
//! Progress is pushed as `ServerEvent::ScanProgress` over a broadcast channel.

pub mod parser;
pub mod probe;
mod scan;
pub mod watcher;

pub use parser::{
    FileClass, LibraryKind, MediaKind, ParsedMedia, SidecarSubtitle, classify, parse_path,
};
pub use probe::{Hdr, ProbeError, ProbeResult, StreamInfo, StreamKind};
pub use scan::{MetadataProvider, ScanSummary, Scanner, library_roots};
pub use watcher::LibraryWatcher;
