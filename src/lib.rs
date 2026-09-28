pub mod config;
pub mod constants;
pub mod decoders;
pub mod engine;
pub mod stream;

pub use config::BotConfig;
pub use decoders::*;
pub use engine::*;
pub use stream::*;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
