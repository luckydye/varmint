use crate::instance::Instance;
use serde::{Deserialize, Serialize};
use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
};

const KERNEL_ARGS: &str =
    "console=ttyAMA0 earlycon=pl011,mmio32,0x09000000 root=LABEL=varmint-root rw quiet loglevel=3 apparmor=0 systemd.condition-needs-update=0";
const MIB: u64 = 1024 * 1024;
pub const GIB: u64 = 1024 * MIB;

/// Virtual hardware of one VM, stored as `meta.json` in its directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hardware {
    pub persist: bool,
    pub cpus: usize,
    pub memory_mib: u64,
    pub disk_gib: u64,
    pub width: u32,
    pub height: u32,
}

impl Hardware {
    pub fn validate(&self) -> Result<(), String> {
        if self.cpus == 0 {
            return Err("cpus must be greater than zero".into());
        }
        if self.memory_mib < 256 {
            return Err("memory must be at least 256 MiB".into());
        }
        if self.disk_gib == 0 {
            return Err("disk must be at least 1 GiB".into());
        }
        if self.width < 320 || self.height < 200 {
            return Err("display size must be at least 320x200".into());
        }
        Ok(())
    }
}

/// A host directory shared into the guest: `HOST:GUEST[:ro]`.
#[derive(Debug, Clone)]
pub struct Mount {
    pub host: PathBuf,
    pub guest: String,
    pub readonly: bool,
}

impl Mount {
    /// Relative guest paths are relative to the guest user's home.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let (spec, readonly) = match spec.strip_suffix(":ro") {
            Some(spec) => (spec, true),
            None => (spec, false),
        };
        let (host, guest) = spec
            .split_once(':')
            .ok_or_else(|| format!("invalid mount {spec:?}; expected HOST:GUEST[:ro]"))?;
        let host = Path::new(host)
            .canonicalize()
            .map_err(|error| format!("invalid mount source {host}: {error}"))?;
        if !host.is_dir() {
            return Err(format!("mount source {} is not a directory", host.display()));
        }
        if guest.is_empty() {
            return Err(format!("invalid mount {spec:?}; the guest path is empty"));
        }
        let guest = match guest.starts_with('/') {
            true => guest.to_owned(),
            false => format!("/home/varmint/{guest}"),
        };
        Ok(Self { host, guest, readonly })
    }

    pub fn spec(&self) -> String {
        let suffix = if self.readonly { ":ro" } else { "" };
        format!("{}:{}{suffix}", self.host.display(), self.guest)
    }
}

#[derive(Debug, Clone)]
pub struct VmConfig {
    pub memory_size: usize,
    pub vcpus: usize,
    pub disk: PathBuf,
    pub disk_size: u64,
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub kernel_args: String,
    pub width: u32,
    pub height: u32,
}

impl VmConfig {
    pub fn new(instance: &Instance, hardware: &Hardware) -> Self {
        let memory_size = usize::try_from(hardware.memory_mib.checked_mul(MIB).expect("memory is too large"))
            .expect("memory is too large for this host");

        Self {
            memory_size,
            vcpus: hardware.cpus,
            disk: instance.disk(),
            disk_size: hardware.disk_gib.checked_mul(GIB).expect("disk is too large"),
            kernel: instance.kernel(),
            initrd: instance.initrd(),
            kernel_args: format!("{KERNEL_ARGS} systemd.hostname={}", instance.name),
            width: hardware.width,
            height: hardware.height,
        }
    }
}

pub struct Resources {
    pub base_image: PathBuf,
    pub kernel: PathBuf,
    pub initrd: PathBuf,
}

impl Resources {
    pub fn locate() -> Self {
        let resources = runtime_dir().map(|runtime| runtime.join("Resources"));
        Self {
            base_image: resource_path(
                resources.as_deref(),
                "runtime/base.raw.zst",
                "build/guest/varmint-debian.raw.zst",
            ),
            kernel: resource_path(resources.as_deref(), "kernel/Image", "build/guest/Image"),
            initrd: resource_path(resources.as_deref(), "kernel/initrd", "build/guest/initrd"),
        }
    }
}

/// Directory holding `Helpers/`, `Frameworks/` and `Resources/`: the app
/// bundle's `Contents`, or `PREFIX/share/varmint` for `PREFIX/bin/varmint`.
/// `None` for development builds run from the cargo target directory.
pub fn runtime_dir() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?.canonicalize().ok()?;
    let directory = executable.parent()?;
    if directory.file_name()? == OsStr::new("MacOS") {
        return Some(directory.parent()?.to_owned());
    }

    let share = directory.parent()?.join("share/varmint");
    share.is_dir().then_some(share)
}

fn resource_path(resources: Option<&Path>, bundled_name: &str, development_path: &str) -> PathBuf {
    resources
        .map(|resources| resources.join(bundled_name))
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(development_path))
}
