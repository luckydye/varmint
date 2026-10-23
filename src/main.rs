mod agent;
mod app;
mod audio;
mod cli;
mod clipboard;
mod config;
mod control;
mod cpu;
mod devices;
mod display;
mod fuse;
mod help;
mod instance;
mod irq;
mod keymap;
mod machine;
mod macos_ui;
mod memory;
mod net;
mod protocol;
mod registry;
mod screen;
mod seed;
mod serve;
mod stdio;
mod telemetry;
mod uart;
mod virtio;
mod vmm;

fn main() {
    cli::main();
}
