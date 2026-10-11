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
mod instance;
mod irq;
mod keymap;
mod machine;
mod macos_ui;
mod memory;
mod net;
mod protocol;
mod stdio;
mod uart;
mod virtio;
mod vmm;

fn main() {
    cli::main();
}
