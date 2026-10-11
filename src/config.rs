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
        let resources = bundle_resources();
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

fn bundle_resources() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?.canonicalize().ok()?;
    let macos = executable.parent()?;
    if macos.file_name()? != OsStr::new("MacOS") {
        return None;
    }

    Some(macos.parent()?.join("Resources"))
}

fn resource_path(resources: Option<&Path>, bundled_name: &str, development_path: &str) -> PathBuf {
    resources
        .map(|resources| resources.join(bundled_name))
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(development_path))
}
