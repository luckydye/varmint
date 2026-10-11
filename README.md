<p align="left">
  <img src="./assets/icon.png" width="100">
</p>

# varmint
Varmint runs quick, disposable Linux VMs on Macs, built on top of Hypervisor.framework.

Every VM is a copy-on-write clone of a Debian arm64 base image with an Xfce desktop and hardware-accelerated graphics. VMs are ephemeral by default and are deleted when they stop; `--persist` keeps them. A guest agent gives the host shells, commands and SSH without network setup, and the CLI drives the screen, mouse and keyboard.

**Varmint requires macOS 26 or later.**

## Usage
Put the CLI on your `PATH`:

```sh
ln -s /Applications/Varmint.app/Contents/MacOS/varmint /usr/local/bin/varmint
```

Start a VM. `run` prints the VM name once the guest agent is up:

```sh
vm=$(varmint run)                  # ephemeral, headless
varmint run dev --persist          # kept across stops, restart with `varmint run dev`
varmint run --window               # with a window and shared clipboard
varmint run --cpus 8 --memory 8192 --disk 32 --size 1920x1080
```

The first `run` decompresses the base image once. Later VMs clone it in milliseconds.

Shells and commands:

```sh
varmint shell $vm
varmint exec $vm -- uname -a
echo hello | varmint exec $vm -- cat
varmint ssh $vm                    # regular ssh, tunneled through the agent
scp -o ProxyCommand="varmint proxy $vm 22" file varmint@$vm:   # any TCP port works
```

Share host directories over virtio-fs. Relative guest paths are relative to `/home/varmint`:

```sh
varmint run --mount ~/project:/work --mount ~/datasets:data:ro
```

Files appear owned by the guest user and changes happen as your macOS user. The guest cannot follow symlinks out of a share.

Screen, mouse and keyboard. Coordinates are screenshot pixels:

```sh
varmint screenshot $vm shot.png
varmint move $vm 640 400
varmint click $vm 640 400 [--right | --middle] [--double]
varmint type $vm "hello world"
varmint key $vm ctrl+alt+t enter
varmint scroll $vm -3
```

Manage VMs:

```sh
varmint ls
varmint stop $vm                   # ephemeral VMs are deleted
varmint rm dev                     # delete a persistent VM
```

The guest user is `varmint` with passwordless `sudo`. VM state lives in `~/.varmint` (override with `VARMINT_HOME`), each VM in `~/.varmint/vms/<name>` with its `vmm.log`. Shutting down or rebooting the guest stops the VM.

## Features

* VMs boot from copy-on-write disk clones
* ephemeral by default, optionally persistent
* shell, exec and TCP tunnels (SSH) through a virtio-console guest agent
* host directories shared over virtio-fs, optionally read-only
* screenshots and mouse/keyboard input from the CLI
* hardware-accelerated Vulkan and OpenGL
* audio, networking and input
* optional window with shared clipboard

## Graphics

Vulkan:

```text
guest Vulkan => Mesa Venus => virglrenderer => MoltenVK => Metal
```

OpenGL:

```text
guest OpenGL => Mesa VirGL => virglrenderer => ANGLE => Metal
```

## Building from source
Requires macOS 26 or later, Xcode, Rust and Docker.

```sh
brew install cmake meson ninja pkg-config python
make app
```

## Acknowledgements
Varmint relies on several forks maintained by the UTM project, particularly in the graphics stack.

Huge thanks to the UTM contributors for making this work possible.

## Resources
* [Booting AArch64 Linux](https://docs.kernel.org/arch/arm64/booting.html)
* [VIRTIO](https://docs.oasis-open.org/virtio/virtio/v1.3/csd01/virtio-v1.3-csd01.html)
