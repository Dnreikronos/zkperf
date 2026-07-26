//! Shared identifiers for the versioned zkperf adapter subprocess protocol.

/// Stable identifier carried by every adapter protocol message.
pub const PROTOCOL_ID: &str = "zkperf-adapter";

/// Adapter protocol versions implemented by this workspace.
pub const SUPPORTED_VERSIONS: &[&str] = &["1.0.0"];

#[cfg(test)]
mod tests {
    use super::{PROTOCOL_ID, SUPPORTED_VERSIONS};

    #[test]
    fn exposes_the_documented_v1_protocol() {
        assert_eq!(PROTOCOL_ID, "zkperf-adapter");
        assert_eq!(SUPPORTED_VERSIONS, ["1.0.0"]);
    }
}
