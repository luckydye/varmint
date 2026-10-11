use crate::{
    config::{Hardware, Mount},
    instance::{self, Instance, Launch},
    keymap,
    protocol::{self, Frame, InputEvent, Reply, Request, Session, kind},
    virtio::input::keys::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, KEY_LEFTSHIFT},
    vmm,
};
use crossterm::terminal;
use std::{
    env,
    fs::{self, File},
    io::{self, BufReader, IsTerminal, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::{net::UnixListener, net::UnixStream, process::CommandExt},
    },
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicI32, Ordering},
    },
    thread,
    time::Instant,
};

const USAGE: &str = "usage:
  varmint run [NAME] [--persist] [--window] [--wait-network] [--mount HOST:GUEST[:ro]]...
              [--cpus N] [--memory MIB] [--disk GIB] [--size WxH]
  varmint ls
  varmint stop NAME
  varmint rm NAME
  varmint shell NAME [-- COMMAND...]
  varmint exec NAME -- COMMAND...
  varmint ssh NAME [SSH ARGS...]
  varmint proxy NAME PORT
  varmint screenshot NAME [FILE]
  varmint move NAME X Y
  varmint click NAME [X Y] [--right | --middle] [--double]
  varmint type NAME TEXT
  varmint key NAME COMBO...         e.g. ctrl+alt+t, enter
  varmint scroll NAME AMOUNT [--horizontal]   positive scrolls up/right";

const DEFAULT_HARDWARE: Hardware = Hardware {
    persist: false,
    cpus: 4,
    memory_mib: 4096,
    disk_gib: 16,
    width: 1280,
    height: 800,
};

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("varmint: {message}");
    std::process::exit(1)
}

pub fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some((command, args)) = args.split_first() else {
        fail(USAGE);
    };

    match command.as_str() {
        "run" => run(args),
        "ls" => list(),
        "stop" => stop(&open(name_arg(args))),
        "rm" => remove(name_arg(args)),
        "shell" => shell(args),
        "exec" => exec(args),
        "ssh" => ssh(args),
        "proxy" => proxy(args),
        "screenshot" => screenshot(args),
        "move" => move_pointer(args),
        "click" => click(args),
        "type" => type_text(args),
        "key" => key(args),
        "scroll" => scroll(args),
        "__vmm" => vmm_process(args),
        "help" | "--help" | "-h" => println!("{USAGE}"),
        _ => fail(USAGE),
    }
}

fn name_arg(args: &[String]) -> &str {
    args.first().unwrap_or_else(|| fail(USAGE))
}

fn open(name: &str) -> Instance {
    Instance::open(name).unwrap_or_else(|error| fail(error))
}

fn parse<T: std::str::FromStr>(value: Option<&String>, what: &str) -> T {
    value
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| fail(format!("invalid {what}")))
}

fn run(args: &[String]) {
    let started = Instant::now();
    let mut name = None;
    let mut window = false;
    let mut wait_network = false;
    let mut mounts = Vec::new();
    let mut hardware = DEFAULT_HARDWARE;
    let mut hardware_flags = false;

    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--window" => window = true,
            "--wait-network" => wait_network = true,
            "--mount" => {
                let spec = args.next().unwrap_or_else(|| fail("--mount needs HOST:GUEST[:ro]"));
                mounts.push(Mount::parse(spec).unwrap_or_else(|error| fail(error)));
            }
            "--persist" => {
                hardware.persist = true;
                hardware_flags = true;
            }
            "--cpus" => {
                hardware.cpus = parse(args.next(), "--cpus");
                hardware_flags = true;
            }
            "--memory" => {
                hardware.memory_mib = parse(args.next(), "--memory");
                hardware_flags = true;
            }
            "--disk" => {
                hardware.disk_gib = parse(args.next(), "--disk");
                hardware_flags = true;
            }
            "--size" => {
                let size = args.next().unwrap_or_else(|| fail("--size needs WxH"));
                let (width, height) = size.split_once('x').unwrap_or_else(|| fail("--size needs WxH"));
                hardware.width = parse(Some(&width.to_owned()), "--size width");
                hardware.height = parse(Some(&height.to_owned()), "--size height");
                hardware_flags = true;
            }
            _ if name.is_none() && !arg.starts_with('-') => name = Some(arg.clone()),
            _ => fail(USAGE),
        }
    }

    instance::prune();
    let launch = match name {
        Some(name) if Instance::open(&name).is_ok() => {
            if hardware_flags {
                fail(format!("VM {name} already exists; its hardware cannot be changed"));
            }
            instance::restart(&name)
        }
        Some(name) => instance::create(&name, &hardware),
        None => {
            let name = format!("vm-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
            instance::create(&name, &hardware)
        }
    }
    .unwrap_or_else(|error| fail(error));

    let name = launch.instance.name.clone();
    let log = launch.instance.log();
    spawn_vmm(launch, window, &mounts);

    let instance = open(&name);
    let (_, reply) = request(&instance, &Request::Wait).unwrap_or_else(|_| {
        let log = fs::read_to_string(&log).unwrap_or_default();
        let tail: Vec<&str> = log.lines().rev().take(20).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        fail(format!("VM {name} exited during boot:\n{}", tail.join("\n")))
    });

    if wait_network {
        wait_for_network(&instance);
    }

    eprintln!(
        "{name} ready in {:.2}s (guest boot {:.2}s)",
        started.elapsed().as_secs_f64(),
        reply.boot_ms.expect("wait reply without boot time") as f64 / 1000.0
    );
    println!("{name}");
}

/// Blocks until the guest has a configured link and a running DNS resolver.
/// Both waits are systemd's own, so nothing here polls.
fn wait_for_network(instance: &Instance) {
    let argv = [
        "sudo",
        "sh",
        "-c",
        "/usr/lib/systemd/systemd-networkd-wait-online --any --timeout=30 && systemctl start systemd-resolved.service",
    ];
    let session = Session::Exec {
        argv: argv.map(str::to_owned).to_vec(),
    };
    let code = run_session(instance, session, false, false);
    if code != 0 {
        fail(format!("network in VM {} did not come up (exit code {code})", instance.name));
    }
}

fn spawn_vmm(launch: Launch, window: bool, mounts: &[Mount]) {
    let lock_fd = launch.lock.as_raw_fd();
    let listen_fd = launch.listener.as_raw_fd();
    set_inheritable(lock_fd, true);
    set_inheritable(listen_fd, true);

    let log = File::create(launch.instance.log()).unwrap_or_else(|error| fail(format!("failed to create log: {error}")));
    // Resolve symlinks: the VM process finds its helpers next to the real binary.
    let executable = env::current_exe()
        .and_then(|path| path.canonicalize())
        .unwrap_or_else(|error| fail(error));
    let mut command = Command::new(executable);
    command
        .arg("__vmm")
        .arg(&launch.instance.name)
        .arg(lock_fd.to_string())
        .arg(listen_fd.to_string())
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap_or_else(|error| fail(error)))
        .stderr(log)
        .process_group(0);
    if window {
        command.arg("--window");
    }
    for mount in mounts {
        command.arg("--mount").arg(mount.spec());
    }
    command
        .spawn()
        .unwrap_or_else(|error| fail(format!("failed to start VM process: {error}")));
}

fn set_inheritable(fd: RawFd, inheritable: bool) {
    let flags = if inheritable { 0 } else { libc::FD_CLOEXEC };
    assert!(unsafe { libc::fcntl(fd, libc::F_SETFD, flags) } == 0, "fcntl failed: {}", io::Error::last_os_error());
}

/// Entry point of the detached VM process started by `varmint run`.
fn vmm_process(args: &[String]) {
    let [name, lock_fd, listen_fd, rest @ ..] = args else {
        fail("internal: __vmm NAME LOCK_FD LISTEN_FD [--window] [--mount SPEC]...");
    };
    let mut window = false;
    let mut mounts = Vec::new();
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--window" => window = true,
            "--mount" => {
                let spec = rest.next().unwrap_or_else(|| fail("internal: --mount needs a spec"));
                mounts.push(Mount::parse(spec).unwrap_or_else(|error| fail(error)));
            }
            _ => fail(format!("internal: unexpected argument {arg}")),
        }
    }
    let lock_fd: RawFd = parse(Some(lock_fd), "lock fd");
    let listen_fd: RawFd = parse(Some(listen_fd), "listen fd");

    // Helpers spawned by the VM must not keep the lock or the socket alive.
    set_inheritable(lock_fd, false);
    set_inheritable(listen_fd, false);
    let launch = Launch {
        instance: open(name),
        lock: unsafe { File::from_raw_fd(lock_fd) },
        listener: unsafe { UnixListener::from_raw_fd(listen_fd) },
    };

    vmm::run(launch, window, &mounts).unwrap_or_else(|error| panic!("VM failed: {error:?}"));
}

fn list() {
    instance::prune();
    println!("{:<20} {:<8} {:<8} {:>4} {:>8} {:>8}", "NAME", "STATE", "PERSIST", "CPUS", "MEMORY", "DISK");
    for instance in instance::list() {
        let hardware = instance.hardware().unwrap_or_else(|error| fail(error));
        println!(
            "{:<20} {:<8} {:<8} {:>4} {:>6}Mi {:>6}Gi",
            instance.name,
            if instance.is_running() { "running" } else { "stopped" },
            if hardware.persist { "yes" } else { "no" },
            hardware.cpus,
            hardware.memory_mib,
            hardware.disk_gib,
        );
    }
}

fn stop(instance: &Instance) {
    let persist = instance.hardware().unwrap_or_else(|error| fail(error)).persist;
    let lock = instance.lock_file();
    if persist {
        let code = run_session(instance, Session::Exec { argv: vec!["sync".into()] }, false, false);
        if code != 0 {
            fail(format!("sync in VM {} failed with exit code {code}", instance.name));
        }
    }
    request(instance, &Request::Stop).unwrap_or_else(|error| fail(error));
    // Blocks until the VM process has exited and released its lock.
    lock.lock().unwrap_or_else(|error| fail(error));
}

fn remove(name: &str) {
    let instance = open(name);
    if instance.is_running() {
        stop(&instance);
    }
    if instance.dir.exists() {
        instance::remove(&instance);
    }
}

fn split_command(args: &[String]) -> Vec<String> {
    match args.iter().position(|arg| arg == "--") {
        Some(index) => args[index + 1..].to_vec(),
        None if args.is_empty() => Vec::new(),
        None => fail(USAGE),
    }
}

fn shell(args: &[String]) {
    let instance = open(name_arg(args));
    let argv = split_command(&args[1..]);
    let interactive = io::stdin().is_terminal();
    let (cols, rows) = if interactive { terminal::size().unwrap_or_else(|error| fail(error)) } else { (80, 24) };
    let session = Session::Pty {
        argv,
        cols,
        rows,
        term: env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
    };
    std::process::exit(run_session(&instance, session, interactive, true));
}

fn exec(args: &[String]) {
    let instance = open(name_arg(args));
    let argv = split_command(&args[1..]);
    if argv.is_empty() {
        fail(USAGE);
    }
    std::process::exit(run_session(&instance, Session::Exec { argv }, false, true));
}

fn proxy(args: &[String]) {
    let [name, port] = args else { fail(USAGE) };
    let instance = open(name);
    let port = parse(Some(port), "port");
    std::process::exit(run_session(&instance, Session::Tcp { port }, false, true));
}

fn ssh(args: &[String]) {
    let name = name_arg(args);
    open(name);
    let key = instance::ssh_key();
    let executable = env::current_exe()
        .and_then(|path| path.canonicalize())
        .unwrap_or_else(|error| fail(error));
    let error = Command::new("ssh")
        .arg("-o")
        .arg(format!("ProxyCommand='{}' proxy {name} 22", executable.display()))
        .args(["-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "LogLevel=ERROR"])
        .arg("-i")
        .arg(key)
        .arg(format!("varmint@{name}"))
        .args(&args[1..])
        .exec();
    fail(format!("failed to run ssh: {error}"))
}

/// Streams stdin/stdout through a guest session and returns its exit code.
fn run_session(instance: &Instance, session: Session, raw: bool, forward_stdin: bool) -> i32 {
    let stream = instance.connect().unwrap_or_else(|error| fail(error));
    let mut writer = stream.try_clone().unwrap_or_else(|error| fail(error));
    protocol::write_json(&mut writer, &Request::Open { session }).unwrap_or_else(|error| fail(error));
    let mut reader = BufReader::new(stream);

    if raw {
        terminal::enable_raw_mode().unwrap_or_else(|error| fail(error));
    }
    let restore = || {
        if raw {
            let _ = terminal::disable_raw_mode();
        }
    };

    if !forward_stdin {
        protocol::write_frame(&mut writer, &Frame::new(0, kind::EOF, Vec::new())).unwrap_or_else(|error| fail(error));
    }
    // Stdin and resize frames come from different threads; whole frames only.
    let writer = Arc::new(Mutex::new(writer));
    if forward_stdin {
        let writer = writer.clone();
        thread::spawn(move || pump_stdin(&writer));
    }
    if raw {
        forward_resizes(writer);
    }

    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    loop {
        let frame = match protocol::read_frame(&mut reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                restore();
                fail("connection to the VM closed")
            }
            Err(error) => {
                restore();
                fail(error)
            }
        };
        match frame.kind {
            kind::DATA => {
                let _ = stdout.write_all(&frame.payload);
                let _ = stdout.flush();
            }
            kind::STDERR => {
                let _ = stderr.write_all(&frame.payload);
            }
            kind::EXIT => {
                restore();
                return i32::from_le_bytes(frame.payload.try_into().expect("malformed exit frame"));
            }
            other => {
                restore();
                fail(format!("unexpected frame kind {other} from the VM"))
            }
        }
    }
}

fn pump_stdin(writer: &Mutex<UnixStream>) {
    let mut stdin = io::stdin();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let frame = match stdin.read(&mut buffer) {
            Ok(0) | Err(_) => Frame::new(0, kind::EOF, Vec::new()),
            Ok(n) => Frame::new(0, kind::DATA, buffer[..n].to_vec()),
        };
        let eof = frame.kind == kind::EOF;
        if protocol::write_frame(&mut *writer.lock().unwrap(), &frame).is_err() || eof {
            return;
        }
    }
}

/// Write end of the self-pipe the SIGWINCH handler signals through.
static RESIZE_PIPE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_window_change(_: libc::c_int) {
    let byte = 0u8;
    // Async-signal-safe; a full pipe already has a resize pending.
    unsafe { libc::write(RESIZE_PIPE.load(Ordering::Relaxed), (&raw const byte).cast(), 1) };
}

/// Sends the terminal size to the guest pty whenever the window changes.
fn forward_resizes(writer: Arc<Mutex<UnixStream>>) {
    let mut fds = [0; 2];
    assert!(unsafe { libc::pipe(fds.as_mut_ptr()) } == 0, "pipe failed: {}", io::Error::last_os_error());
    let [read_fd, write_fd] = fds;
    assert!(unsafe { libc::fcntl(write_fd, libc::F_SETFL, libc::O_NONBLOCK) } == 0);
    RESIZE_PIPE.store(write_fd, Ordering::Relaxed);
    let handler = on_window_change as extern "C" fn(libc::c_int);
    assert!(
        unsafe { libc::signal(libc::SIGWINCH, handler as libc::sighandler_t) } != libc::SIG_ERR,
        "failed to install the SIGWINCH handler"
    );

    thread::spawn(move || {
        let mut pipe = unsafe { File::from_raw_fd(read_fd) };
        let mut signal = [0u8; 64];
        while matches!(pipe.read(&mut signal), Ok(n) if n > 0) {
            let Ok((cols, rows)) = terminal::size() else {
                continue;
            };
            let mut payload = cols.to_le_bytes().to_vec();
            payload.extend_from_slice(&rows.to_le_bytes());
            if protocol::write_frame(&mut *writer.lock().unwrap(), &Frame::new(0, kind::RESIZE, payload)).is_err() {
                return;
            }
        }
    });
}

fn request(instance: &Instance, request: &Request) -> Result<(BufReader<UnixStream>, Reply), String> {
    let mut stream = instance.connect()?;
    protocol::write_json(&mut stream, request).map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(stream);
    let reply: Reply = protocol::read_json(&mut reader).map_err(|error| error.to_string())?;
    match reply.error {
        Some(error) => Err(error),
        None => Ok((reader, reply)),
    }
}

fn send_input(name: &str, events: Vec<InputEvent>) {
    request(&open(name), &Request::Input { events }).unwrap_or_else(|error| fail(error));
}

fn screenshot(args: &[String]) {
    let name = name_arg(args);
    let path = args.get(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("{name}.png")));
    let (mut reader, reply) = request(&open(name), &Request::Screenshot).unwrap_or_else(|error| fail(error));
    let mut png = vec![0; reply.png_len.expect("screenshot reply without length")];
    reader.read_exact(&mut png).unwrap_or_else(|error| fail(error));

    if path.as_os_str() == "-" {
        io::stdout().write_all(&png).unwrap_or_else(|error| fail(error));
    } else {
        fs::write(&path, png).unwrap_or_else(|error| fail(format!("failed to write {}: {error}", path.display())));
        println!("{}", path.display());
    }
}

fn move_pointer(args: &[String]) {
    let [name, x, y] = args else { fail(USAGE) };
    send_input(
        name,
        vec![InputEvent::Move {
            x: parse(Some(x), "x"),
            y: parse(Some(y), "y"),
        }],
    );
}

fn click(args: &[String]) {
    let name = name_arg(args);
    let mut button = BTN_LEFT;
    let mut clicks = 1;
    let mut position = Vec::new();
    for arg in &args[1..] {
        match arg.as_str() {
            "--right" => button = BTN_RIGHT,
            "--middle" => button = BTN_MIDDLE,
            "--double" => clicks = 2,
            _ => position.push(parse::<u32>(Some(arg), "coordinate")),
        }
    }

    let mut events = match position.as_slice() {
        [] => Vec::new(),
        [x, y] => vec![InputEvent::Move { x: *x, y: *y }],
        _ => fail(USAGE),
    };
    for _ in 0..clicks {
        events.push(InputEvent::Button { button, down: true });
        events.push(InputEvent::Button { button, down: false });
    }
    send_input(name, events);
}

fn type_text(args: &[String]) {
    let [name, text] = args else { fail(USAGE) };
    let mut events = Vec::new();
    for c in text.chars() {
        let (code, shift) = keymap::char_key(c).unwrap_or_else(|| fail(format!("cannot type {c:?}")));
        if shift {
            events.push(InputEvent::Key {
                code: KEY_LEFTSHIFT,
                down: true,
            });
        }
        events.push(InputEvent::Key { code, down: true });
        events.push(InputEvent::Key { code, down: false });
        if shift {
            events.push(InputEvent::Key {
                code: KEY_LEFTSHIFT,
                down: false,
            });
        }
    }
    send_input(name, events);
}

fn key(args: &[String]) {
    let name = name_arg(args);
    if args.len() < 2 {
        fail(USAGE);
    }

    let mut events = Vec::new();
    for combo in &args[1..] {
        let codes: Vec<u16> = combo
            .split('+')
            .map(|key| keymap::named_key(key).unwrap_or_else(|| fail(format!("unknown key {key:?}"))))
            .collect();
        events.extend(codes.iter().map(|&code| InputEvent::Key { code, down: true }));
        events.extend(codes.iter().rev().map(|&code| InputEvent::Key { code, down: false }));
    }
    send_input(name, events);
}

fn scroll(args: &[String]) {
    let name = name_arg(args);
    let value = parse(args.get(1), "scroll amount");
    let horizontal = args[2..].iter().any(|arg| arg == "--horizontal");
    send_input(name, vec![InputEvent::Scroll { value, horizontal }]);
}
