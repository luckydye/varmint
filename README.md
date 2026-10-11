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

`varmint help` lists all commands and `varmint COMMAND --help` explains one with its options and examples.

Start a VM. `run` prints the VM name once the guest agent is up:

```sh
vm=$(varmint run)                  # ephemeral, headless
varmint run dev --persist          # kept across stops, restart with `varmint run dev`
varmint run --window               # with a window and shared clipboard
varmint run --wait-network         # return once the guest has network and DNS
varmint run --cpus 8 --memory 8192 --disk 32 --size 1920x1080
```

The first `run` decompresses the base image once. Later VMs clone it in milliseconds. `run` returns as soon as the guest agent is up, about a second after start; the network follows a moment later unless you pass `--wait-network`.

Shells and commands:

```sh
varmint exec $vm                   # interactive shell
varmint exec $vm -- htop           # interactive programs get a terminal
varmint exec $vm -- uname -a
echo hello | varmint exec $vm -- cat   # in pipes: no terminal, separate stdout and stderr
varmint ssh $vm                    # regular ssh, tunneled through the agent
scp -o ProxyCommand="varmint proxy $vm 22" file varmint@$vm:   # any TCP port works
```

Share host directories over virtio-fs. Relative guest paths are relative to `/home/varmint`:

```sh
varmint run --mount ~/project:/work --mount ~/datasets:data:ro
```

Files appear owned by the guest user and changes happen as your macOS user. The guest cannot follow symlinks out of a share.

Give every VM environment variables, such as an agent's API token, with `KEY=VALUE` lines in `~/.varmint/env` (must be `chmod 600`), or per run with `--env KEY=VALUE` (`--env KEY` copies it from your shell). They are set in `exec` and `ssh` sessions; programs in the VM can read them.

Copy host files or directories into every VM at start, such as an agent's skills or settings, with `HOST:GUEST` lines in `~/.varmint/seeds`, or per run with `--seed HOST:GUEST`. `~/` in HOST is your home and relative GUEST paths are under `/home/varmint`. A directory in a git work tree leaves out what `.gitignore` ignores but keeps `.git`. It is a one-way copy: changes in the VM never reach the host.

```sh
# ~/.varmint/seeds
~/.claude/skills:.claude/skills
~/.gitconfig:.gitconfig
```

Run a single command in a fresh VM; the VM stops afterwards and an ephemeral one is deleted:

```sh
varmint run --mount .:work -- make -C work test
```

Add your own commands as executables in `~/.varmint/commands`: `varmint NAME` runs `~/.varmint/commands/NAME` with the remaining arguments and `$VARMINT` set to the varmint binary. For example, `varmint dev` to open Claude Code on a copy of the current directory in a throwaway VM:

```sh
#!/bin/sh
# ~/.varmint/commands/dev
exec varmint run --seed .:work --wait-network -- bash -lc 'cd work && mise use -g -q claude && claude "$@"' claude "$@"
```

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

## Telemetry

Every VM records what it does in `~/.varmint/telemetry.sqlite`, so you can see which programs run too often or get installed repeatedly and are worth preinstalling:

* `vms`: start, boot time, stop time and stop reason (`stopped`, `guest-poweroff`, `window-closed`, `crash`) of every run
* `processes`: every process that exited in the guest, with wall and CPU time (kernel process accounting; names are truncated to 15 characters)
* `commands`: command lines typed in interactive guest shells (`shell`) or run with `varmint exec` (`exec`)
* `packages`: apt packages and mise tools installed in the guest

```sh
sqlite3 ~/.varmint/telemetry.sqlite

-- programs by total wall time
SELECT command, count(*) AS runs, round(sum(elapsed_ms) / 1000, 1) AS seconds
FROM processes GROUP BY command ORDER BY seconds DESC LIMIT 20;

-- packages installed in more than one VM: candidates for the base image
SELECT manager, name, count(DISTINCT vm_id) AS vms FROM packages
GROUP BY manager, name HAVING vms > 1 ORDER BY vms DESC;

-- VM uptime
SELECT name, boot_ms, round(stopped_at - started_at) AS seconds, stop_reason FROM vms ORDER BY id DESC;
```

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
