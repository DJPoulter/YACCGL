use std::time::Duration;

pub const USER_AGENT: &str = concat!("YetAnotherCreatureCollectorGameLauncher/", env!("CARGO_PKG_VERSION"));

/// Shared HTTP agent. Only the connect and response-header phases have timeouts,
/// so large downloads on slow connections are never cut off mid-stream.
pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .build()
        .into()
}
