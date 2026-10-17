//! `varmint serve`: VM provisioning over TCP, one JSON request and one JSON
//! reply per connection. Requests run the varmint CLI, so a failing request
//! cannot take the server down and behaves exactly like the command.
//!
//! The log on stderr is JSON Lines of CloudEvents 1.0, for programs to act
//! on. Events of a connection carry the extension attributes `peer` (client
//! address) and `requestid`, shared by all events of one connection. Types, with
//! their `data`:
//!
//! - `dev.varmint.serve.request`: the request as received, without its token
//!   and with only the names of `env`
//! - `dev.varmint.serve.exec`: `args`, `exit_code` (null if killed by a
//!   signal) and `duration_ms` of a finished varmint command
//! - `dev.varmint.serve.reply`: the reply as sent
//! - `dev.varmint.serve.error`: `message` of a connection that failed, such
//!   as a client that left before the reply, or of a failed accept, which has
//!   no `peer` or `requestid`

mod web;

use crate::{
    config::{GUEST_ENVIRONMENT_VARIABLE, Hardware},
    instance, protocol,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env,
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::UnixStream,
    },
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// How long a client may take to send its whole request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Requests are a few hundred bytes; a longer line is cut off and fails to parse.
const MAX_REQUEST_BYTES: u64 = 64 * 1024;

/// Pause after a failed accept, so a lasting error such as running out of
/// file descriptors does not spin the loop.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Hardware fields only apply when a VM is created, like the `run` flags.
/// `env` holds the guest variables, such as the client's own
/// CLAUDE_CODE_OAUTH_TOKEN; they reach `varmint run` in
/// GUEST_ENVIRONMENT_VARIABLE, never its arguments or its own variables.
/// Mounts and seeds are left out on purpose: they name host files, which a
/// remote client must not reach.
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
enum Request {
    Run {
        name: Option<String>,
        base: Option<String>,
        #[serde(default)]
        persist: bool,
        cpus: Option<usize>,
        memory_mib: Option<u64>,
        disk_gib: Option<u64>,
        size: Option<(u32, u32)>,
        #[serde(default)]
        wait_network: bool,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Ls,
    Stop {
        name: String,
    },
    Rm {
        name: String,
    },
    /// Hands the connection to the VM's control socket with `request` as its
    /// first line, for screen streams, input and shells.
    Attach {
        name: String,
        request: protocol::Request,
    },
}

#[derive(Serialize, Default)]
struct Reply {
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vms: Option<Vec<Vm>>,
}

#[derive(Serialize)]
struct Vm {
    name: String,
    running: bool,
    hardware: Hardware,
}

pub fn token_path() -> PathBuf {
    instance::home().join("serve-token")
}

/// The token clients must send, created on first use.
pub fn token() -> Result<String, String> {
    let path = token_path();
    if !path.exists() {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
        file.write_all(uuid::Uuid::new_v4().simple().to_string().as_bytes())
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }
    let metadata = fs::metadata(&path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "{} must only be readable by you; run: chmod 600 {}",
            path.display(),
            path.display()
        ));
    }
    let token = fs::read_to_string(&path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let token = token.trim().to_owned();
    if token.is_empty() {
        return Err(format!("{} is empty", path.display()));
    }
    Ok(token)
}

/// With `ui`, connections that do not start with a JSON request are HTTP,
/// answered by [`web`] with the web UI and its API.
pub fn serve(listener: TcpListener, token: &str, ui: bool) -> ! {
    thread::scope(|scope| {
        loop {
            let (connection, peer) = match listener.accept() {
                Ok(accepted) => accepted,
                Err(error) => {
                    emit("error", None, json!({ "message": format!("accept failed: {error}") }));
                    thread::sleep(ACCEPT_RETRY_DELAY);
                    continue;
                }
            };
            let client = Client {
                peer,
                request_id: uuid::Uuid::new_v4().to_string(),
            };
            scope.spawn(move || {
                if let Err(error) = handle(connection, &client, token, ui) {
                    client.emit("error", json!({ "message": error.to_string() }));
                }
            });
        }
    })
}

struct Client {
    peer: SocketAddr,
    request_id: String,
}

impl Client {
    fn emit(&self, kind: &str, data: Value) {
        emit(kind, Some(self), data);
    }
}

/// Writes one CloudEvent as a line on stderr.
fn emit(kind: &str, client: Option<&Client>, data: Value) {
    let mut event = json!({
        "specversion": "1.0",
        "id": uuid::Uuid::new_v4().to_string(),
        "source": "varmint/serve",
        "type": format!("dev.varmint.serve.{kind}"),
        "time": rfc3339(SystemTime::now()),
        "data": data,
    });
    if let Some(client) = client {
        event["peer"] = json!(client.peer.to_string());
        event["requestid"] = json!(client.request_id);
    }
    protocol::write_json(&mut io::stderr().lock(), &event).expect("failed to write the serve log");
}

/// Reads from a client until a deadline for the whole request, so a client
/// that sends a byte at a time cannot hold its connection open.
struct Deadline {
    stream: TcpStream,
    deadline: Instant,
}

impl Read for Deadline {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(timed_out());
        }
        self.stream.set_read_timeout(Some(remaining))?;
        // An expired read timeout is WouldBlock on macOS.
        self.stream.read(buffer).map_err(|error| match error.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => timed_out(),
            _ => error,
        })
    }
}

fn timed_out() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!("not sent within {}s", REQUEST_TIMEOUT.as_secs()),
    )
}

/// UTC time as RFC 3339 with milliseconds, such as 2026-10-17T12:00:00.123Z.
fn rfc3339(time: SystemTime) -> String {
    let since_epoch = time
        .duration_since(UNIX_EPOCH)
        .expect("host clock is before the unix epoch");
    let seconds = since_epoch.as_secs();
    // Days since 1970-01-01 to a civil date, Howard Hinnant's days_from_civil inverted.
    let days = seconds / 86400 + 719468;
    let era = days / 146097;
    let day_of_era = days - era * 146097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
        since_epoch.subsec_millis()
    )
}

fn handle(connection: TcpStream, client: &Client, token: &str, ui: bool) -> io::Result<()> {
    let deadline = Deadline {
        stream: connection.try_clone()?,
        deadline: Instant::now() + REQUEST_TIMEOUT,
    };
    let mut reader = BufReader::new(deadline.take(MAX_REQUEST_BYTES));
    let mut writer = connection;
    if ui && reader.fill_buf()?.first().is_some_and(|&byte| byte != b'{') {
        return web::handle(client, reader, writer, token);
    }
    let request = protocol::read_json(&mut reader).map_err(|error| format!("unreadable request: {error}"));
    let (reply, control) = respond(client, request, token);
    protocol::write_json(&mut writer, &reply)?;
    match control {
        Some(control) => relay(reader, writer, control),
        None => Ok(()),
    }
}

/// Answers a request and logs the reply; for `attach`, also returns the
/// connected control socket.
fn respond(client: &Client, request: Result<Value, String>, token: &str) -> (Reply, Option<UnixStream>) {
    let (reply, control) = match request.and_then(|request| authorize(client, request, token)) {
        Ok(Request::Attach { name, request }) => match attach(&name, &request) {
            Ok(control) => (Ok(Reply::default()), Some(control)),
            Err(error) => (Err(error), None),
        },
        request => (request.and_then(|request| execute(client, request)), None),
    };
    let reply = reply.unwrap_or_else(|error| Reply {
        error: Some(error),
        ..Reply::default()
    });
    client.emit("reply", serde_json::to_value(&reply).expect("a reply serializes"));
    (reply, control)
}

fn authorize(client: &Client, mut request: Value, token: &str) -> Result<Request, String> {
    let given = request.as_object_mut().and_then(|fields| fields.remove("token"));
    let mut logged = request.clone();
    if let Some(env) = logged.get_mut("env") {
        *env = json!(env.as_object().map(|variables| variables.keys().collect::<Vec<_>>()));
    }
    client.emit("request", logged);
    match given
        .as_ref()
        .and_then(Value::as_str)
        .is_some_and(|given| token_matches(given, token))
    {
        true => serde_json::from_value(request).map_err(|error| format!("invalid request: {error}")),
        false => Err("invalid token".to_owned()),
    }
}

/// Compares in constant time, so response times do not reveal the token.
fn token_matches(given: &str, token: &str) -> bool {
    given.len() == token.len() && given.bytes().zip(token.bytes()).fold(0, |diff, (a, b)| diff | (a ^ b)) == 0
}

fn execute(client: &Client, request: Request) -> Result<Reply, String> {
    match request {
        Request::Run {
            name,
            base,
            persist,
            cpus,
            memory_mib,
            disk_gib,
            size,
            wait_network,
            env,
        } => {
            let mut args = vec!["run".to_owned()];
            if let Some(name) = name {
                instance::validate_name(&name, "VM")?;
                args.push(name);
            }
            if let Some(base) = base {
                instance::validate_name(&base, "image")?;
                args.extend(["--base".to_owned(), base]);
            }
            if persist {
                args.push("--persist".into());
            }
            if let Some(cpus) = cpus {
                args.extend(["--cpus".to_owned(), cpus.to_string()]);
            }
            if let Some(memory_mib) = memory_mib {
                args.extend(["--memory".to_owned(), memory_mib.to_string()]);
            }
            if let Some(disk_gib) = disk_gib {
                args.extend(["--disk".to_owned(), disk_gib.to_string()]);
            }
            if let Some((width, height)) = size {
                args.extend(["--size".to_owned(), format!("{width}x{height}")]);
            }
            if wait_network {
                args.push("--wait-network".into());
            }
            let name = varmint(client, &args, &env)?;
            Ok(Reply {
                name: Some(name),
                ..Reply::default()
            })
        }
        Request::Ls => {
            instance::prune();
            let vms = instance::list()
                .into_iter()
                .map(|instance| -> Result<Vm, String> {
                    Ok(Vm {
                        running: instance.is_running(),
                        hardware: instance.hardware()?,
                        name: instance.name,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Reply {
                vms: Some(vms),
                ..Reply::default()
            })
        }
        Request::Stop { name } => {
            instance::validate_name(&name, "VM")?;
            varmint(client, &["stop".to_owned(), name], &BTreeMap::new()).map(|_| Reply::default())
        }
        Request::Rm { name } => {
            instance::validate_name(&name, "VM")?;
            varmint(client, &["rm".to_owned(), name], &BTreeMap::new()).map(|_| Reply::default())
        }
        Request::Attach { .. } => unreachable!("attach is handled before execute"),
    }
}

/// Connects to the control socket of VM `name` and sends `request`.
fn attach(name: &str, request: &protocol::Request) -> Result<UnixStream, String> {
    instance::validate_name(name, "VM")?;
    let mut control = instance::Instance::open(name)?.connect()?;
    protocol::write_json(&mut control, request).map_err(|error| error.to_string())?;
    Ok(control)
}

/// Passes bytes between the client and a control socket until either side
/// closes, starting with what the client sent after its request line.
fn relay(mut reader: BufReader<io::Take<Deadline>>, mut writer: TcpStream, mut control: UnixStream) -> io::Result<()> {
    control.write_all(reader.buffer())?;
    reader.consume(reader.buffer().len());
    writer.set_read_timeout(None)?;
    let mut from_client = writer.try_clone()?;
    let mut to_client = control.try_clone()?;
    thread::scope(|scope| {
        scope.spawn(move || {
            let _ = io::copy(&mut to_client, &mut writer);
            let _ = writer.shutdown(Shutdown::Both);
        });
        let result = io::copy(&mut from_client, &mut control);
        let _ = control.shutdown(Shutdown::Write);
        result.map(|_| ())
    })
}

/// Runs a varmint command with `environment` as its guest variables and
/// returns its stdout, or its error message without the `varmint: ` prefix,
/// like the other errors of a reply.
fn varmint(client: &Client, args: &[String], environment: &BTreeMap<String, String>) -> Result<String, String> {
    let executable = env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| error.to_string())?;
    let started = Instant::now();
    let output = Command::new(executable)
        .args(args)
        .env(
            GUEST_ENVIRONMENT_VARIABLE,
            serde_json::to_string(environment).expect("variables serialize"),
        )
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("failed to run varmint: {error}"))?;
    client.emit(
        "exec",
        json!({
            "args": args,
            "exit_code": output.status.code(),
            "duration_ms": started.elapsed().as_millis() as u64,
        }),
    );
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        return Err(message.strip_prefix("varmint: ").unwrap_or(message).to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
