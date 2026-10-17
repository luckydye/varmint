use crate::{
    agent, app, audio, clipboard,
    config::{self, Mount, Seed, VmConfig},
    fuse,
    control,
    devices::{HostBackends, Runtime, RuntimeEvent},
    display::{DisplayBuffer, DisplayNotifier},
    instance::{self, Launch},
    machine::*,
    memory, net,
    protocol::{GuestMount, GuestSeeds},
    seed,
    stdio, telemetry, virtio,
};
use applevisor::prelude::*;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{Receiver, Sender},
    },
    thread,
};

struct BootPayload {
    kernel: Vec<u8>,
    initrd: Vec<u8>,
}

impl BootPayload {
    fn load(config: &VmConfig) -> Self {
        let kernel = fs::read(&config.kernel)
            .unwrap_or_else(|error| panic!("failed to read kernel {}: {error}", config.kernel.display()));
        let initrd = fs::read(&config.initrd)
            .unwrap_or_else(|error| panic!("failed to read initrd {}: {error}", config.initrd.display()));
        Self { kernel, initrd }
    }
}

/// Where the guest mounts the staged seeds while copying them.
const SEED_MOUNT: &str = "/run/varmint/seed";

/// Directory deleted when an ephemeral VM exits.
static EPHEMERAL_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Ends the VM process, deleting the VM if it is ephemeral.
pub fn shutdown(reason: &'static str) -> ! {
    telemetry::stop(reason);
    if let Some(dir) = EPHEMERAL_DIR.get() {
        if let Err(error) = instance::archive_log(dir) {
            eprintln!("failed to keep the VM log: {error}");
        }
        let _ = fs::remove_dir_all(dir);
    }
    std::process::exit(0)
}

fn vmm_thread(
    vm: &VirtualMachineInstance<GicEnabled>,
    display: &Mutex<DisplayBuffer>,
    runtime_event_tx: Sender<RuntimeEvent>,
    runtime_event_rx: Receiver<RuntimeEvent>,
    audio_event_rx: Receiver<audio::BackendEvent>,
    display_notifier: DisplayNotifier,
    config: &VmConfig,
    boot: BootPayload,
    backends: HostBackends,
    gicd_size: u64,
    gicr_size: u64,
) -> Result<()> {
    let shares = backends.shares.len();
    let initrd_range = Some((INITRD_START, checked_end(INITRD_START, boot.initrd.len(), "initrd")));
    let dtb = build_fdt(
        config.memory_size as u64,
        config.vcpus,
        &config.kernel_args,
        initrd_range,
        gicd_size,
        gicr_size,
        shares,
    )
    .unwrap_or_else(|error| panic!("failed to build guest FDT: {error}"));

    validate_boot_layout(config.memory_size, boot.kernel.len(), &boot.initrd, dtb.len());

    let mem = memory::GuestMemory::new(RAM_START, config.memory_size);
    mem.write(IMAGE_START, &boot.kernel)?;
    mem.write(INITRD_START, &boot.initrd)?;
    mem.write(DTB_START, &dtb)?;
    // The VMM runs until the VM stops; the guest has its own copy now.
    drop(boot);

    Runtime::new(vm, runtime_event_tx, backends, config.vcpus)?.run(
        &mem,
        display,
        runtime_event_rx,
        audio_event_rx,
        &display_notifier,
    )?;

    Ok(())
}

fn checked_end(start: u64, len: usize, name: &str) -> u64 {
    start
        .checked_add(len as u64)
        .unwrap_or_else(|| panic!("{name} address range overflows"))
}

fn validate_boot_layout(memory_size: usize, image_size: usize, initrd: &[u8], dtb_size: usize) {
    let ram_end = checked_end(RAM_START, memory_size, "guest RAM");
    let image_end = checked_end(IMAGE_START, image_size, "kernel");
    if image_end > INITRD_START {
        panic!("kernel ends at 0x{image_end:x}, overlapping initrd region at 0x{INITRD_START:x}");
    }

    let initrd_end = checked_end(INITRD_START, initrd.len(), "initrd");
    if initrd_end > DTB_START {
        panic!("initrd ends at 0x{initrd_end:x}, overlapping FDT at 0x{DTB_START:x}");
    }

    let dtb_end = checked_end(DTB_START, dtb_size, "FDT");
    if dtb_end > ram_end {
        panic!("FDT ends at 0x{dtb_end:x}, past guest RAM end at 0x{ram_end:x}");
    }
}

pub fn run(
    launch: Launch,
    window: bool,
    mounts: &[Mount],
    seeds: &[Seed],
    environment: BTreeMap<String, String>,
) -> Result<()> {
    let Launch {
        instance,
        lock: _lock,
        listener,
    } = launch;
    let hardware = instance.hardware().unwrap_or_else(|error| panic!("{error}"));
    let mut config = VmConfig::new(&instance, &hardware);
    if window {
        // Tells the guest to sync the clipboard; headless VMs skip it.
        config.kernel_args.push_str(" varmint.window=1");
    }
    telemetry::start(&instance.name, &hardware, mounts.len(), window);

    if !hardware.persist {
        EPHEMERAL_DIR.set(instance.dir.clone()).unwrap();
    }
    // A panic on any thread ends the VM. Its directory stays for `varmint run` to
    // show the log; the next prune deletes it if the VM is ephemeral.
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_panic(info);
        telemetry::stop("crash");
        std::process::exit(101);
    }));

    if let Some(runtime) = config::runtime_dir() {
        if std::env::var_os("RENDER_SERVER_EXEC_PATH").is_none() {
            let render_server = runtime.join("Helpers/virgl_render_server");
            if render_server.is_file() {
                unsafe {
                    std::env::set_var("RENDER_SERVER_EXEC_PATH", render_server);
                }
            }
        }
    }

    let disk = virtio::Blk::new(&config.disk, config.disk_size)
        .unwrap_or_else(|error| panic!("failed to open VM disk {}: {error}", config.disk.display()));
    let boot = BootPayload::load(&config);

    let gicd_size = GicConfig::get_distributor_size()? as u64;
    let gicr_region_size = GicConfig::get_redistributor_region_size()? as u64;
    let gicr_stride = GicConfig::get_redistributor_size()? as u64;
    assert!(gicr_stride != 0);

    let max_vcpus = gicr_region_size / gicr_stride;
    if config.vcpus as u64 > max_vcpus {
        panic!(
            "vcpus={} exceeds the Hypervisor.framework GIC limit of {max_vcpus}",
            config.vcpus
        );
    }
    assert!(GICR_START > GICD_START + gicd_size);
    let gicr_size = gicr_stride * config.vcpus as u64;
    assert!(UART_START >= GICR_START + gicr_size);

    let mut gic_config = GicConfig::new();
    gic_config.set_distributor_base(GICD_START)?;
    gic_config.set_redistributor_base(GICR_START)?;

    let mut vm_cfg = VirtualMachineConfig::new();
    vm_cfg.set_ipa_granule(IpaGranule::HV_IPA_GRANULE_4KB)?;
    let vm = VirtualMachine::with_gic(vm_cfg, gic_config)?;

    let display = Mutex::new(DisplayBuffer::new());

    let (runtime_event_tx, runtime_event_rx) = std::sync::mpsc::channel();

    let staged_seeds =
        seed::stage(&instance, seeds).unwrap_or_else(|error| panic!("failed to stage seeds: {error}"));
    let mut mounts = mounts.to_vec();
    if let Some((staging, _)) = &staged_seeds {
        mounts.push(Mount {
            host: staging.clone(),
            guest: SEED_MOUNT.into(),
            readonly: true,
        });
    }
    assert!(mounts.len() <= MAX_SHARES, "at most {MAX_SHARES} shares are supported");

    let shares = mounts
        .iter()
        .enumerate()
        .map(|(index, mount)| {
            let share = fuse::Share::new(&mount.host, mount.readonly)
                .unwrap_or_else(|error| panic!("failed to open share {}: {error}", mount.host.display()));
            virtio::fs_channel(&format!("share{index}"), share)
        })
        .collect();
    let guest_mounts = mounts
        .iter()
        .enumerate()
        .map(|(index, mount)| GuestMount {
            tag: format!("share{index}"),
            path: mount.guest.clone(),
            readonly: mount.readonly,
        })
        .collect();

    let (agent, agent_sink) = agent::start(
        runtime_event_tx.clone(),
        instance::authorized_keys(),
        guest_mounts,
        environment,
        staged_seeds.map(|(_, items)| GuestSeeds {
            mount: SEED_MOUNT.into(),
            items,
        }),
    );

    let net_rx_tx = runtime_event_tx.clone();
    let network = net::start(move |frame| {
        let _ = net_rx_tx.send(RuntimeEvent::NetRx(frame));
    })
    .unwrap_or_else(|error| panic!("failed to start vmnet networking: {error}"));

    // Only a VM with a window plays on the host's speakers; every VM streams its
    // audio to control clients.
    let audio_listeners = Arc::new(audio::Listeners::default());
    let (audio_event_tx, audio_event_rx) = std::sync::mpsc::channel();
    let (_audio_backend, period_sink) = audio::Backend::new(window, Arc::clone(&audio_listeners), move |event| {
        let _ = audio_event_tx.send(event);
    })
    .unwrap_or_else(|error| panic!("failed to start audio output: {error}"));

    // Only a VM with a window syncs the host clipboard; headless isolates only get
    // clipboard images from interactive `varmint exec` sessions.
    let clipboard = window.then(|| {
        let clipboard_change_tx = runtime_event_tx.clone();
        clipboard::start(move |payload| {
            let _ = clipboard_change_tx.send(RuntimeEvent::Clipboard(payload));
        })
    });

    let serial_rx_tx = runtime_event_tx.clone();
    let serial = stdio::start(move |byte| {
        let _ = serial_rx_tx.send(RuntimeEvent::UartRx(byte));
    });

    let backends = HostBackends {
        disk,
        net: network,
        audio: period_sink,
        clipboard,
        agent: agent_sink,
        serial,
        shares,
    };

    let control_context = control::Context {
        display: &display,
        agent: &agent,
        audio: &audio_listeners,
        runtime_tx: runtime_event_tx.clone(),
        display_size: (config.width, config.height),
    };

    let event_loop = window.then(app::event_loop);
    let display_notifier = match &event_loop {
        Some(event_loop) => DisplayNotifier::Window(event_loop.create_proxy()),
        None => DisplayNotifier::Headless,
    };

    thread::scope(|scope| {
        let vmm_runtime_event_tx = runtime_event_tx.clone();
        let vm = &vm;
        let display = &display;
        let config = &config;
        scope.spawn(move || {
            vmm_thread(
                vm,
                display,
                vmm_runtime_event_tx,
                runtime_event_rx,
                audio_event_rx,
                display_notifier,
                config,
                boot,
                backends,
                gicd_size,
                gicr_size,
            )
            .unwrap_or_else(|error| panic!("VM failed: {error:?}"));
            shutdown("exited")
        });

        match event_loop {
            Some(event_loop) => {
                let control_context = &control_context;
                scope.spawn(move || control::serve(listener, control_context));
                app::run(event_loop, display, runtime_event_tx, (config.width, config.height));
                shutdown("window-closed")
            }
            None => {
                runtime_event_tx
                    .send(RuntimeEvent::DisplayResized {
                        physical_width: config.width,
                        physical_height: config.height,
                        logical_width: config.width,
                        logical_height: config.height,
                        refresh_millihertz: 60_000,
                    })
                    .expect("runtime event pump exited");
                control::serve(listener, &control_context)
            }
        }
    })
}
