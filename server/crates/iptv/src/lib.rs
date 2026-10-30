//! IPTV: M3U/Xtream ingest, XMLTV EPG, shared upstream fan-out, GOP cache, warm channels.

pub mod epg;
pub mod error;
pub mod hls;
pub mod hub;
pub mod ingest;
pub mod live;
pub mod m3u;
pub mod net;
pub mod service;
pub mod store;
pub mod ts;
pub mod xmltv;
pub mod xtream;

pub use error::{Error, Result};
pub use hls::HlsOptions;
pub use hub::{
    Candidate, ChannelHub, ChannelMetrics, ChannelResolver, ChannelSpec, HubConfig, Subscription,
};
pub use live::{LiveManager, LiveMode, LiveSessionInfo};
pub use service::{IptvConfig, IptvService};
