//! `varmint serve`: VM provisioning over TCP, one JSON request and one JSON
//! reply per connection. Requests run the varmint CLI, so a failing request
//! cannot take the server down and behaves exactly like the command.
//!
//! Every request, every command it runs and every reply is logged to stderr
//! with the time and the client address; tokens are never logged.

use crate::{config::Hardware, instance, protocol, telemetry};
use serde::{Deserialize, Serialize};
use std::{
    env,
    fmt::Display,
    fs::{self, OpenOptions},
    io::{self, BufReader, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// How long a client may take to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct Envelope {
    token: String,
    #[serde(flatten)]
    request: Request,
}

/// Hardware fields only apply when a VM is created, like the `run` flags.
/// Mounts, seeds and environment are left out on purpose: they name host
/// files, which a remote client must not reach.
#[derive(Deserialize, Debug)]
#[serde(tag = "op", rename_all = "lowercase")]
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
    },
    Ls,
    Stop {
        name: String,
    },
    Rm {
        name: String,
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

pub fn serve(listener: TcpListener, token: &str) -> ! {
    thread::scope(|scope| {
        for connection in listener.incoming() {
            let connection = connection.unwrap_or_else(|error| panic!("serve: accept failed: {error}"));
            scope.spawn(move || match connection.peer_addr() {
                Ok(peer) => {
                    if let Err(error) = handle(connection, peer, token) {
                        log(peer, format!("client error: {error}"));
                    }
                }
                Err(error) => eprintln!("serve: client error: {error}"),
            });
        }
    });
    unreachable!("serve listener stopped accepting connections")
}

/// One line on stderr: unix time, client address, message.
fn log(peer: SocketAddr, message: impl Display) {
    eprintln!("serve: {:.3} {peer} {message}", telemetry::now());
}

fn handle(connection: TcpStream, peer: SocketAddr, token: &str) -> io::Result<()> {
    connection.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    let mut reader = BufReader::new(connection.try_clone()?);
    let mut writer = connection;
    let envelope: Envelope = protocol::read_json(&mut reader)?;
    log(peer, format!("request {:?}", envelope.request));
    let reply = match token_matches(&envelope.token, token) {
        true => execute(peer, envelope.request),
        false => Err("invalid token".to_owned()),
    };
    match &reply {
        Ok(_) => log(peer, "reply ok"),
        Err(error) => log(peer, format!("reply error: {error}")),
    }
    let reply = reply.unwrap_or_else(|error| Reply {
        error: Some(error),
        ..Reply::default()
    });
    protocol::write_json(&mut writer, &reply)
}

/// Compares in constant time, so response times do not reveal the token.
fn token_matches(given: &str, token: &str) -> bool {
    given.len() == token.len() && given.bytes().zip(token.bytes()).fold(0, |diff, (a, b)| diff | (a ^ b)) == 0
}

fn execute(peer: SocketAddr, request: Request) -> Result<Reply, String> {
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
            let name = varmint(peer, &args)?;
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
            varmint(peer, &["stop".to_owned(), name]).map(|_| Reply::default())
        }
        Request::Rm { name } => {
            instance::validate_name(&name, "VM")?;
            varmint(peer, &["rm".to_owned(), name]).map(|_| Reply::default())
        }
    }
}

/// Runs a varmint command and returns its stdout, or its error message.
fn varmint(peer: SocketAddr, args: &[String]) -> Result<String, String> {
    let executable = env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| error.to_string())?;
    log(peer, format!("exec varmint {}", args.join(" ")));
    let started = Instant::now();
    let output = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("failed to run varmint: {error}"))?;
    log(
        peer,
        format!("exec {} in {:.2}s", output.status, started.elapsed().as_secs_f64()),
    );
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
