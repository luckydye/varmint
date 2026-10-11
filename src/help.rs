//! Text of `varmint help`, kept apart from the commands it documents.

use crate::config::Hardware;

pub struct Command {
    pub name: &'static str,
    group: &'static str,
    usage: &'static str,
    summary: &'static str,
    description: String,
    options: Vec<(&'static str, String)>,
    examples: &'static [(&'static str, &'static str)],
}

const GROUPS: &[&str] = &["VMs", "Access", "Screen and input"];

const OVERVIEW_EXAMPLES: &[(&str, &str)] = &[
    ("vm=$(varmint run --mount .:work)", "start a VM with the current directory at ~/work"),
    ("varmint exec $vm -- ls work", "run a command in it"),
    ("varmint screenshot $vm screen.png", "look at its screen"),
    ("varmint stop $vm", "stop and delete it"),
];

pub fn commands(defaults: &Hardware) -> Vec<Command> {
    vec![
        Command {
            name: "run",
            group: "VMs",
            usage: "varmint run [NAME] [OPTIONS]",
            summary: "Create and start a VM, or start a stopped persistent one",
            description: "\
Creates a VM from the base image and boots it in the background. Once the
guest is ready, about a second later, the VM name is printed on stdout, so
`vm=$(varmint run)` works in scripts. NAME defaults to a random vm-XXXXXX.

Without --persist the VM is ephemeral and deleted when it stops. Running
`varmint run NAME` for a stopped persistent VM starts it again with its disk
intact; hardware options only apply when a VM is created."
                .into(),
            options: vec![
                ("--persist", "Keep the VM and its disk after it stops".into()),
                ("--window", "Show the screen in a window and share the clipboard".into()),
                (
                    "--wait-network",
                    "Return only once the guest has network and DNS (about a second more)".into(),
                ),
                (
                    "--mount HOST:GUEST[:ro]",
                    "Share a host directory at GUEST; relative GUEST paths are under\n\
                     /home/varmint and :ro makes the share read-only. Repeatable, up to 8"
                        .into(),
                ),
                ("--cpus N", format!("Virtual CPUs (default {})", defaults.cpus)),
                ("--memory MIB", format!("Memory in MiB (default {})", defaults.memory_mib)),
                ("--disk GIB", format!("Disk size in GiB (default {})", defaults.disk_gib)),
                (
                    "--size WxH",
                    format!("Screen size in pixels (default {}x{})", defaults.width, defaults.height),
                ),
            ],
            examples: &[
                ("vm=$(varmint run)", "ephemeral VM without a window"),
                ("varmint run dev --persist", "persistent VM named dev"),
                ("varmint run dev", "start dev again after it stopped"),
                ("varmint run --mount .:work --wait-network", "current directory at ~/work, network ready"),
                ("varmint run --window --size 1920x1080", "VM with a window"),
            ],
        },
        Command {
            name: "ls",
            group: "VMs",
            usage: "varmint ls",
            summary: "List VMs",
            description: "Lists all VMs with their state, whether they persist, and their hardware.".into(),
            options: vec![],
            examples: &[("varmint ls", "")],
        },
        Command {
            name: "stop",
            group: "VMs",
            usage: "varmint stop NAME",
            summary: "Stop a VM; ephemeral VMs are deleted",
            description: "\
Stops a running VM. Persistent VMs flush their disk first and can be started
again with `varmint run NAME`; ephemeral VMs are deleted."
                .into(),
            options: vec![],
            examples: &[("varmint stop $vm", "")],
        },
        Command {
            name: "rm",
            group: "VMs",
            usage: "varmint rm NAME",
            summary: "Delete a VM, stopping it first if it runs",
            description: "Deletes a VM and its disk. A running VM is stopped first.".into(),
            options: vec![],
            examples: &[("varmint rm dev", "")],
        },
        Command {
            name: "shell",
            group: "Access",
            usage: "varmint shell NAME [-- COMMAND...]",
            summary: "Open an interactive shell, or run a command on a terminal",
            description: "\
Starts a login shell as the guest user varmint on a real terminal, so
interactive programs like editors and top work. With COMMAND, runs that
instead of the shell. The terminal size follows your window, and varmint
exits with the shell's exit code."
                .into(),
            options: vec![],
            examples: &[
                ("varmint shell $vm", "interactive shell"),
                ("varmint shell $vm -- htop", "full-screen program"),
            ],
        },
        Command {
            name: "exec",
            group: "Access",
            usage: "varmint exec NAME -- COMMAND...",
            summary: "Run a command without a terminal, for scripts and pipes",
            description: "\
Runs COMMAND as the guest user varmint in /home/varmint. stdin, stdout and
stderr are passed through separately and varmint exits with the command's
exit code. Use sudo for root; it needs no password."
                .into(),
            options: vec![],
            examples: &[
                ("varmint exec $vm -- uname -a", ""),
                ("echo hello | varmint exec $vm -- cat", "stdin is forwarded"),
                ("varmint exec $vm -- sudo apt-get install -y ripgrep", "run as root"),
            ],
        },
        Command {
            name: "ssh",
            group: "Access",
            usage: "varmint ssh NAME [SSH ARGS...]",
            summary: "Connect with ssh, tunneled through the guest agent",
            description: "\
Runs your ssh client as varmint@NAME through `varmint proxy`, so it needs no
network setup. Your ~/.ssh/*.pub keys and a key in ~/.varmint/ssh are
authorized in every VM. Remaining arguments are passed to ssh."
                .into(),
            options: vec![],
            examples: &[
                ("varmint ssh $vm", "interactive session"),
                ("varmint ssh $vm uptime", "run a command"),
                ("varmint ssh $vm -L 8080:localhost:8080", "forward a port"),
            ],
        },
        Command {
            name: "proxy",
            group: "Access",
            usage: "varmint proxy NAME PORT",
            summary: "Connect stdin and stdout to a TCP port in a VM",
            description: "\
Pipes stdin and stdout to PORT on the guest's localhost, for use as an ssh
ProxyCommand or to talk to a guest service. A service that is still starting
gets up to 30 seconds to accept the connection."
                .into(),
            options: vec![],
            examples: &[
                (
                    "scp -o ProxyCommand=\"varmint proxy $vm 22\" file varmint@$vm:",
                    "copy a file with scp",
                ),
                (
                    "printf 'GET / HTTP/1.0\\r\\n\\r\\n' | varmint proxy $vm 8080",
                    "talk to a guest web server",
                ),
            ],
        },
        Command {
            name: "screenshot",
            group: "Screen and input",
            usage: "varmint screenshot NAME [FILE]",
            summary: "Save the screen as a PNG",
            description: "\
Writes the VM's screen to FILE, NAME.png by default, or to stdout when FILE
is -. Pixel positions in the image are the coordinates move and click take."
                .into(),
            options: vec![],
            examples: &[
                ("varmint screenshot $vm", "writes $vm.png"),
                ("varmint screenshot $vm - > screen.png", "PNG on stdout"),
            ],
        },
        Command {
            name: "move",
            group: "Screen and input",
            usage: "varmint move NAME X Y",
            summary: "Move the mouse pointer",
            description: "Moves the pointer to screenshot pixel X Y.".into(),
            options: vec![],
            examples: &[("varmint move $vm 640 400", "")],
        },
        Command {
            name: "click",
            group: "Screen and input",
            usage: "varmint click NAME [X Y] [--right | --middle] [--double]",
            summary: "Click a mouse button",
            description: "Clicks at screenshot pixel X Y, or where the pointer is without X Y.".into(),
            options: vec![
                ("--right", "Click the right button".into()),
                ("--middle", "Click the middle button".into()),
                ("--double", "Double-click".into()),
            ],
            examples: &[
                ("varmint click $vm 568 775", "left click"),
                ("varmint click $vm 300 200 --right", "open a context menu"),
                ("varmint click $vm 60 80 --double", "open a desktop icon"),
            ],
        },
        Command {
            name: "type",
            group: "Screen and input",
            usage: "varmint type NAME TEXT",
            summary: "Type text",
            description: "\
Types TEXT with a US keyboard layout. Printable ASCII, newlines and tabs are
supported; use `varmint key` for other keys and shortcuts."
                .into(),
            options: vec![],
            examples: &[
                ("varmint type $vm 'hello world'", ""),
                ("varmint type $vm $'ls -la\\n'", "type a command and press enter"),
            ],
        },
        Command {
            name: "key",
            group: "Screen and input",
            usage: "varmint key NAME COMBO...",
            summary: "Press keys and shortcuts",
            description: "\
Presses each COMBO in turn. A combo is one or more keys joined with +, held
down together. Keys are ctrl, shift, alt, super, enter, esc, tab, space,
backspace, delete, insert, home, end, pageup, pagedown, up, down, left,
right, f1 to f12, or a single character."
                .into(),
            options: vec![],
            examples: &[
                ("varmint key $vm enter", ""),
                ("varmint key $vm ctrl+alt+t", "shortcut"),
                ("varmint key $vm ctrl+a ctrl+c", "several combos in a row"),
            ],
        },
        Command {
            name: "scroll",
            group: "Screen and input",
            usage: "varmint scroll NAME AMOUNT [--horizontal]",
            summary: "Scroll the mouse wheel",
            description: "\
Scrolls AMOUNT wheel steps at the pointer. Positive scrolls up, negative
scrolls down."
                .into(),
            options: vec![("--horizontal", "Scroll sideways; positive scrolls right".into())],
            examples: &[
                ("varmint scroll $vm -5", "scroll down"),
                ("varmint scroll $vm 3 --horizontal", "scroll right"),
            ],
        },
    ]
}

fn render_examples(out: &mut String, examples: &[(&str, &str)]) {
    let width = examples.iter().map(|(command, _)| command.len()).max().unwrap_or(0);
    for (command, comment) in examples {
        match comment.is_empty() {
            true => out.push_str(&format!("  {command}\n")),
            false => out.push_str(&format!("  {command:<width$}  # {comment}\n")),
        }
    }
}

pub fn overview(commands: &[Command]) -> String {
    let mut out = String::from(
        "\
varmint - quick, disposable Linux VMs on macOS

Every VM is a copy-on-write clone of a Debian desktop image and starts in
about a second. VMs are ephemeral unless created with --persist. State and
usage telemetry live in ~/.varmint (set VARMINT_HOME to move it).

Usage: varmint COMMAND [ARGS]
",
    );
    let width = commands.iter().map(|command| command.name.len()).max().unwrap_or(0);
    for group in GROUPS {
        out.push_str(&format!("\n{group}:\n"));
        for command in commands.iter().filter(|command| command.group == *group) {
            out.push_str(&format!("  {:<width$}  {}\n", command.name, command.summary));
        }
    }
    out.push_str("\nRun `varmint help COMMAND` or `varmint COMMAND --help` for options and examples.\n\nExamples:\n");
    render_examples(&mut out, OVERVIEW_EXAMPLES);
    out
}

pub fn command(command: &Command) -> String {
    let mut out = format!("{}\n\nUsage: {}\n\n{}\n", command.summary, command.usage, command.description);
    if !command.options.is_empty() {
        out.push_str("\nOptions:\n");
        let width = command.options.iter().map(|(flag, _)| flag.len()).max().unwrap_or(0);
        for (flag, text) in &command.options {
            let indent = " ".repeat(width + 4);
            let text = text.replace('\n', &format!("\n{indent}"));
            out.push_str(&format!("  {flag:<width$}  {text}\n"));
        }
    }
    out.push_str("\nExamples:\n");
    render_examples(&mut out, command.examples);
    out
}

/// Short message for wrong arguments to a command.
pub fn usage_error(command: &Command) -> String {
    format!(
        "usage: {}\nRun `varmint {} --help` for options and examples.",
        command.usage, command.name
    )
}
