fn main() {
    let protocols = zkperf_core::supported_adapter_protocol_versions().join(", ");
    println!(
        "zkperf {} (adapter protocols: {protocols})",
        env!("CARGO_PKG_VERSION")
    );
}
