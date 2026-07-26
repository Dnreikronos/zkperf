//! Engine-independent benchmark orchestration primitives.

/// Adapter protocol versions that the core can negotiate.
#[must_use]
pub const fn supported_adapter_protocol_versions() -> &'static [&'static str] {
    zkperf_adapter_protocol::SUPPORTED_VERSIONS
}

#[cfg(test)]
mod tests {
    use super::supported_adapter_protocol_versions;

    #[test]
    fn supports_adapter_protocol_v1() {
        assert_eq!(supported_adapter_protocol_versions(), ["1.0.0"]);
    }
}
