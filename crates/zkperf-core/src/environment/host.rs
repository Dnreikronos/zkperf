use std::collections::BTreeSet;
use std::num::NonZeroU64;

use sysinfo::{CpuRefreshKind, MemoryRefreshKind, System};

use crate::{
    Architecture, CpuMetadata, HostMetadata, HostMetadataParts, NonEmptyString, Observed,
    OperatingSystemMetadata,
};

pub(super) fn text(value: Option<&str>) -> Observed<NonEmptyString> {
    value
        .filter(|value| !value.trim().eq_ignore_ascii_case("unknown"))
        .and_then(|value| NonEmptyString::new(value.trim()).ok())
        .map_or_else(
            || {
                Observed::gap(
                    "not_exposed",
                    "The metadata source did not expose this field.",
                )
            },
            Observed::Available,
        )
}

fn count(value: Option<u64>) -> Observed<NonZeroU64> {
    value.and_then(NonZeroU64::new).map_or_else(
        || {
            Observed::gap(
                "not_exposed",
                "The host API did not expose a positive count.",
            )
        },
        Observed::Available,
    )
}

pub(super) fn collect() -> HostMetadata {
    let mut system = System::new();
    system.refresh_cpu_list(CpuRefreshKind::nothing());
    system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    let brands: BTreeSet<_> = system.cpus().iter().map(|cpu| cpu.brand().trim()).collect();
    let model = if brands.contains("") {
        String::new()
    } else {
        brands.into_iter().collect::<Vec<_>>().join("; ")
    };
    HostMetadata::new(HostMetadataParts {
        machine_id: Observed::gap(
            "excluded",
            "Persistent machine identifiers are not collected.",
        ),
        architecture: architecture(),
        cpu: CpuMetadata::new(
            text(Some(&model)),
            cpu_field("stepping"),
            physical_cores(),
            count(system.cpus().len().try_into().ok()),
        ),
        ram_bytes: count(Some(system.total_memory())),
        accelerators: accelerators(),
        storage: Observed::gap(
            "not_collected",
            "Storage inventory is not collected by this version.",
        ),
        operating_system: OperatingSystemMetadata::new(
            text(System::name().as_deref()),
            text(System::os_version().as_deref()),
            text(System::kernel_version().as_deref()),
        ),
        firmware_or_microcode: Some(cpu_field("microcode")),
    })
}

fn architecture() -> Observed<Architecture> {
    #[cfg(unix)]
    {
        let system = rustix::system::uname();
        match system.machine().to_str().ok() {
            Some("x86_64") => Architecture::X86_64.into(),
            Some("aarch64" | "arm64") => Architecture::Aarch64.into(),
            Some("riscv64") => Architecture::Riscv64.into(),
            Some("wasm32") => Architecture::Wasm32.into(),
            None | Some("" | "unknown") => {
                Observed::gap("not_exposed", "Host architecture is unavailable.")
            }
            Some(_) => Architecture::Other.into(),
        }
    }
    #[cfg(not(unix))]
    Observed::gap(
        "unsupported_platform",
        "Host architecture detection is unavailable; the build target is recorded separately.",
    )
}

fn physical_cores() -> Observed<NonZeroU64> {
    if cfg!(target_os = "linux") {
        let source = std::fs::read_to_string("/proc/cpuinfo").ok();
        count(source.as_deref().and_then(linux_physical_cores))
    } else {
        count(System::physical_core_count().and_then(|value| value.try_into().ok()))
    }
}

fn linux_physical_cores(source: &str) -> Option<u64> {
    let mut cores = BTreeSet::new();
    for processor in source.split("\n\n") {
        let fields: std::collections::BTreeMap<_, _> = processor
            .lines()
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.trim(), value.trim()))
            .collect();
        if fields.contains_key("processor") {
            // Logical processors are not evidence of physical core topology.
            let socket = fields.get("physical id")?.parse::<u64>().ok()?;
            let core = fields.get("core id")?.parse::<u64>().ok()?;
            cores.insert((socket, core));
        }
    }
    (!cores.is_empty()).then_some(cores.len() as u64)
}

fn cpu_field(name: &str) -> Observed<NonEmptyString> {
    if !cfg!(target_os = "linux") {
        return Observed::gap(
            "unsupported_platform",
            "CPU stepping and microcode collection requires Linux procfs.",
        );
    }
    let source = std::fs::read_to_string("/proc/cpuinfo").ok();
    let values: BTreeSet<_> = source
        .as_deref()
        .unwrap_or("")
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == name).then_some(value.trim())
        })
        .collect();
    text(Some(&values.into_iter().collect::<Vec<_>>().join("; ")))
}

fn accelerators() -> Observed<Vec<NonEmptyString>> {
    if !cfg!(target_os = "linux") {
        return Observed::gap(
            "unsupported_platform",
            "Accelerator inventory requires Linux PCI sysfs; no absence is inferred.",
        );
    }
    pci_inventory(std::path::Path::new("/sys/bus/pci/devices")).map_or_else(
        |_| {
            Observed::gap(
                "not_readable",
                "PCI accelerator inventory is inaccessible or incomplete.",
            )
        },
        Observed::Available,
    )
}

fn pci_inventory(root: &std::path::Path) -> std::io::Result<Vec<NonEmptyString>> {
    let mut devices = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        let class = pci_hex(&path.join("class"))?;
        if class >> 16 != 0x03 && class >> 16 != 0x12 {
            continue;
        }
        let vendor = pci_hex(&path.join("vendor"))?;
        let device = pci_hex(&path.join("device"))?;
        devices.push(
            NonEmptyString::new(format!(
                "PCI class={class:06x} vendor={vendor:04x} device={device:04x}"
            ))
            .unwrap(),
        );
    }
    devices.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    Ok(devices)
}

fn pci_hex(path: &std::path::Path) -> std::io::Result<u32> {
    let source = std::fs::read_to_string(path)?;
    u32::from_str_radix(source.trim().trim_start_matches("0x"), 16)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid PCI identifier"))
}

#[cfg(test)]
mod tests {
    use super::{count, linux_physical_cores, pci_inventory, text};
    use crate::Observed;
    use std::fs;

    #[test]
    fn missing_host_fields_remain_explicit() {
        for value in [None, Some(""), Some("  "), Some("Unknown")] {
            assert!(matches!(text(value), Observed::Unavailable(_)));
        }
        for value in [None, Some(0)] {
            assert!(matches!(count(value), Observed::Unavailable(_)));
        }
    }

    #[test]
    fn physical_cores_require_complete_socket_and_core_ids() {
        let source = "processor: 0\nphysical id: 0\ncore id: 0\n\nprocessor: 1\nphysical id: 0\ncore id: 0\n\nprocessor: 2\nphysical id: 1\ncore id: 0";
        assert_eq!(linux_physical_cores(source), Some(2));
        for source in [
            "",
            "processor: 0",
            "processor: 0\ncore id: 0",
            "processor: 0\nphysical id: 0\ncore id: -1",
        ] {
            assert_eq!(linux_physical_cores(source), None);
        }
        assert_eq!(
            linux_physical_cores(&format!("{source}\n\nprocessor: 3")),
            None
        );
    }

    #[test]
    fn pci_inventory_distinguishes_no_devices_from_incomplete_discovery() {
        let root = std::env::temp_dir().join(format!("zkperf-pci-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        assert!(pci_inventory(&root.join("missing")).is_err());
        assert!(pci_inventory(&root).unwrap().is_empty());
        for (name, class) in [("b", "0x030000"), ("a", "0x120000"), ("c", "0x020000")] {
            let path = root.join(name);
            fs::create_dir(&path).unwrap();
            fs::write(path.join("class"), class).unwrap();
            fs::write(path.join("vendor"), "0x10de\n").unwrap();
            fs::write(path.join("device"), "0x1234\n").unwrap();
        }
        let devices = pci_inventory(&root).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(
            devices[0].as_str(),
            "PCI class=030000 vendor=10de device=1234"
        );
        fs::write(root.join("b/class"), "garbage").unwrap();
        assert!(pci_inventory(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
