pub mod detector;
pub mod io;
pub mod logger;
pub mod slack;

use anyhow::Result;

pub struct BridgeConfig {
    pub slack_bot_token: String,
    pub slack_app_token: String,
    pub slack_channel_id: String,
    pub notify_delay_secs: u64,
    pub session_id: String,
    pub project_name: String,
}

pub async fn run(
    _config: BridgeConfig,
    _runtime: &crate::runtime::DockerRuntime,
    _container_id: &str,
) -> Result<i64> {
    todo!("bridge implementation")
}
