//! Per-VM control socket served by the VMM process.

use crate::{
    agent::Agent,
    devices::{RuntimeEvent, RuntimeInputEvent},
    display::DisplayBuffer,
    protocol::{self, Frame, InputEvent, Reply, Request, Session, kind},
    vmm,
};
use std::{
    io::{self, BufReader, Write},
    net::Shutdown,
    os::unix::net::{UnixListener, UnixStream},
    sync::{Mutex, mpsc::Sender},
    thread,
    time::Duration,
};

/// Pause between injected events. Without it a long batch overflows the
/// guest's evdev buffers and the guest drops the whole burst.
const INPUT_EVENT_INTERVAL: Duration = Duration::from_millis(2);

pub struct Context<'a> {
    pub display: &'a Mutex<DisplayBuffer>,
    pub agent: &'a Agent,
    pub runtime_tx: Sender<RuntimeEvent>,
}

pub fn serve(listener: UnixListener, context: &Context<'_>) -> ! {
    thread::scope(|scope| {
        for connection in listener.incoming() {
            let connection = connection.unwrap_or_else(|error| panic!("control socket accept failed: {error}"));
            scope.spawn(move || {
                if let Err(error) = handle(connection, context) {
                    eprintln!("control: client error: {error}");
                }
            });
        }
    });
    unreachable!("control listener stopped accepting connections")
}

fn handle(connection: UnixStream, context: &Context<'_>) -> io::Result<()> {
    let mut reader = BufReader::new(connection.try_clone()?);
    let mut writer = connection;

    match protocol::read_json::<Request>(&mut reader)? {
        Request::Wait => match context.agent.wait_booted() {
            Ok(boot_time) => protocol::write_json(
                &mut writer,
                &Reply {
                    boot_ms: Some(boot_time.as_millis() as u64),
                    ..Reply::default()
                },
            ),
            Err(error) => reply_error(&mut writer, format!("guest setup failed: {error}")),
        },
        Request::Screenshot => {
            let capture = context.display.lock().unwrap().capture();
            let png = capture
                .ok_or_else(|| "the guest has not configured a display yet".to_owned())
                .and_then(|capture| capture.encode_png());
            match png {
                Ok(png) => {
                    protocol::write_json(
                        &mut writer,
                        &Reply {
                            png_len: Some(png.len()),
                            ..Reply::default()
                        },
                    )?;
                    writer.write_all(&png)
                }
                Err(error) => reply_error(&mut writer, error),
            }
        }
        Request::Input { events } => {
            for event in events {
                inject(event, context);
                thread::sleep(INPUT_EVENT_INTERVAL);
            }
            protocol::write_json(&mut writer, &Reply::default())
        }
        Request::Open { session } => relay(reader, writer, &session, context.agent),
        Request::Stop => {
            protocol::write_json(&mut writer, &Reply::default())?;
            vmm::shutdown()
        }
    }
}

fn reply_error(writer: &mut UnixStream, error: String) -> io::Result<()> {
    protocol::write_json(
        writer,
        &Reply {
            error: Some(error),
            ..Reply::default()
        },
    )
}

fn inject(event: InputEvent, context: &Context<'_>) {
    let event = match event {
        InputEvent::Move { x, y } => {
            let (width, height) = context.display.lock().unwrap().size();
            RuntimeInputEvent::PointerMove {
                x,
                y,
                width: width as u32,
                height: height as u32,
            }
        }
        InputEvent::Button { button, down } => RuntimeInputEvent::PointerButton {
            button,
            pressed: down,
            relative: false,
        },
        InputEvent::Key { code, down } => RuntimeInputEvent::Key { code, pressed: down },
        InputEvent::Scroll { value, horizontal } => RuntimeInputEvent::Scroll {
            horizontal,
            value,
            relative: false,
        },
    };
    context
        .runtime_tx
        .send(RuntimeEvent::Input(event))
        .expect("runtime event pump exited");
}

/// Bridges a client connection to one guest agent stream until either side ends.
fn relay(mut reader: BufReader<UnixStream>, writer: UnixStream, session: &Session, agent: &Agent) -> io::Result<()> {
    let (stream, from_guest) = agent.open(session);

    thread::scope(|scope| {
        scope.spawn(move || {
            let mut writer = writer;
            for frame in from_guest {
                if protocol::write_frame(&mut writer, &frame).is_err() || frame.kind == kind::EXIT {
                    break;
                }
            }
            let _ = writer.shutdown(Shutdown::Both);
        });

        let result = loop {
            match protocol::read_frame(&mut reader) {
                Ok(Some(frame)) if matches!(frame.kind, kind::DATA | kind::EOF | kind::RESIZE) => {
                    agent.send(&Frame::new(stream, frame.kind, frame.payload))
                }
                Ok(Some(frame)) => {
                    break Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("clients may not send frame kind {}", frame.kind),
                    ));
                }
                Ok(None) => break Ok(()),
                Err(error) => break Err(error),
            }
        };
        agent.close(stream);
        result
    })
}
