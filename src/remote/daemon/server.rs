// Adapted from cmux-gtk daemon/remote/cmd/cmuxd-remote/main.go (AGPL-3.0-or-later,
// Copyright (c) 2026 Douglas Soares de Andrade). Changes: Use attachment-size aggregation for a Unix-socket session backed by a persistent Rust PTY.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty};

use super::protocol::{self, byte, Size, ATTACH, CHUNK, EXIT, INPUT, KILL, OUTPUT, RESIZE, STATUS};
use super::runtime::Runtime;

pub const REPLAY_LIMIT: usize = 256 * 1024;
const MAX_CLIENTS: usize = 16;
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Default)]
pub struct ReplayBuffer(VecDeque<u8>);

impl ReplayBuffer {
    pub fn push(&mut self, bytes: &[u8]) {
        let bytes = &bytes[bytes.len().saturating_sub(REPLAY_LIMIT)..];
        let discard = (self.0.len() + bytes.len()).saturating_sub(REPLAY_LIMIT);
        self.0.drain(..discard);
        self.0.extend(bytes);
    }

    pub fn snapshot(&self) -> Vec<u8> {
        self.0.iter().copied().collect()
    }
}

type SharedSession = Arc<Mutex<Session>>;
type Output = Arc<[u8]>;

/// Hold a session slot until both I/O workers have finished.
struct ClientSlot {
    _clients: Arc<()>,
}

struct Attachment {
    size: Size,
    stream: UnixStream,
    output: SyncSender<Output>,
}

struct Session {
    master: Box<dyn MasterPty + Send>,
    size: Size,
    replay: ReplayBuffer,
    attachments: HashMap<u64, Attachment>,
    exit: Option<u8>,
}

impl Session {
    /// Use the smallest attached grid; when everyone detaches, the last size stays set.
    fn resize(&mut self) {
        let Some(size) = self
            .attachments
            .values()
            .map(|client| client.size)
            .reduce(|a, b| Size {
                cols: a.cols.min(b.cols),
                rows: a.rows.min(b.rows),
            })
        else {
            return;
        };
        if size != self.size && self.master.resize(size.pty()).is_ok() {
            self.size = size;
        }
    }

    fn publish(&mut self, bytes: &[u8]) {
        if self.exit.is_some() {
            return;
        }
        self.replay.push(bytes);
        if self.attachments.is_empty() {
            return;
        }
        let output: Output = bytes.into();
        let before = self.attachments.len();
        self.attachments.retain(|_, client| {
            if client.output.try_send(output.clone()).is_ok() {
                true
            } else {
                // A stalled attachment must not stop the shell or other attachments.
                let _ = client.stream.shutdown(Shutdown::Both);
                false
            }
        });
        if self.attachments.len() != before {
            self.resize();
        }
    }
}

pub(super) fn serve(runtime: &Runtime, name: &str, size: Size) -> Result<()> {
    let lock = runtime.lock()?;
    let Some(socket) = runtime
        .bind(name)
        .context("binding the remote session socket")?
    else {
        return Ok(());
    };

    let home = std::env::var_os("HOME").context("HOME is required to start a login shell")?;
    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let mut command = CommandBuilder::new(shell);
    command.arg("-l");
    command.cwd(home);
    command.env_remove("TWITTY_REMOTE_MARKER");

    let pair = native_pty_system()
        .openpty(size.pty())
        .context("opening a remote PTY")?;
    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let mut child = pair
        .slave
        .spawn_command(command)
        .context("starting the remote login shell")?;
    drop(pair.slave);
    drop(lock);

    let session = Arc::new(Mutex::new(Session {
        master: pair.master,
        size,
        replay: ReplayBuffer::default(),
        attachments: HashMap::new(),
        exit: None,
    }));
    let reader_state = session.clone();
    let reader = thread::Builder::new()
        .name("twitty-remote-reader".into())
        .spawn(move || {
            let mut buffer = [0; CHUNK];
            while let Ok(n @ 1..) = reader.read(&mut buffer) {
                reader_state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .publish(&buffer[..n]);
            }
        })
        .context("starting remote PTY reader thread")?;
    let (input, input_rx) = mpsc::sync_channel::<Vec<u8>>(32);
    thread::Builder::new()
        .name("twitty-remote-writer".into())
        .spawn(move || {
            for bytes in input_rx {
                if writer
                    .write_all(&bytes)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        })
        .context("starting remote PTY writer thread")?;

    let (kill, kill_rx) = mpsc::sync_channel(1);
    let clients = Arc::new(());
    let mut next_id = 0;
    let mut exited = None;
    let exit_code = loop {
        if exited.is_none() {
            if kill_rx.try_recv().is_ok() {
                let _ = child.kill();
            }
            if let Some(status) = child.try_wait()? {
                exited = Some((status.exit_code().min(255) as u8, Instant::now()));
            }
        }
        if let Some((code, since)) = exited {
            if reader.is_finished() || since.elapsed() >= IO_TIMEOUT {
                break code;
            }
        } else {
            for _ in 0..MAX_CLIENTS {
                match socket.listener.accept() {
                    Ok((stream, _)) if Arc::strong_count(&clients) <= MAX_CLIENTS => {
                        next_id += 1;
                        let slot = Arc::new(ClientSlot {
                            _clients: clients.clone(),
                        });
                        let session = session.clone();
                        let input = input.clone();
                        let kill = kill.clone();
                        let _ = thread::Builder::new()
                            .name("twitty-remote-client".into())
                            .spawn(move || {
                                let _ = handle_client(stream, next_id, session, input, kill, slot);
                            });
                    }
                    Ok(_) => break,
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                    Err(err) => return Err(err.into()),
                }
            }
        }
        thread::sleep(POLL_INTERVAL);
    };

    drop(socket);
    {
        let mut session = session.lock().unwrap_or_else(|e| e.into_inner());
        session.exit = Some(exit_code);
        session.attachments.clear();
    }
    let deadline = Instant::now() + IO_TIMEOUT;
    while Arc::strong_count(&clients) > 1 && Instant::now() < deadline {
        thread::sleep(POLL_INTERVAL);
    }
    Ok(())
}

fn handle_client(
    mut stream: UnixStream,
    id: u64,
    session: SharedSession,
    input: SyncSender<Vec<u8>>,
    kill: SyncSender<()>,
    slot: Arc<ClientSlot>,
) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    match byte(&mut stream)? {
        STATUS => stream.write_all(&[STATUS]),
        KILL => {
            let _ = kill.try_send(());
            stream.write_all(&[KILL])
        }
        ATTACH => {
            let size = Size::read(&mut stream)?;
            stream.set_read_timeout(None)?;
            attach_client(stream, id, size, session, input, slot)
        }
        _ => Err(protocol::invalid("unknown session request")),
    }
}

fn attach_client(
    mut stream: UnixStream,
    id: u64,
    size: Size,
    session: SharedSession,
    input: SyncSender<Vec<u8>>,
    slot: Arc<ClientSlot>,
) -> io::Result<()> {
    let connection = stream.try_clone()?;
    let reader = stream.try_clone()?;
    let (output, queue) = mpsc::sync_channel(32);
    let replay = {
        let mut session = session.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(exit) = session.exit {
            return stream.write_all(&[ATTACH, EXIT, exit]);
        }
        session.attachments.insert(
            id,
            Attachment {
                size,
                stream: connection,
                output,
            },
        );
        session.resize();
        session.replay.snapshot()
    };
    let state = session.clone();
    let input_slot = slot.clone();
    if let Err(err) = thread::Builder::new()
        .name("twitty-remote-input".into())
        .spawn(move || {
            let _slot = input_slot;
            let mut reader = reader;
            let _ = receive_input(&mut reader, id, &state, &input);
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            state.attachments.remove(&id);
            state.resize();
            drop(state);
            // On EOF, stop any output worker blocked writing to the old attachment.
            let _ = reader.shutdown(Shutdown::Both);
        })
    {
        let mut state = session.lock().unwrap_or_else(|e| e.into_inner());
        state.attachments.remove(&id);
        state.resize();
        return Err(err);
    }
    let result = send_output(&mut stream, replay, queue, &session);
    let _ = stream.shutdown(Shutdown::Both);
    result
}

fn receive_input(
    stream: &mut UnixStream,
    id: u64,
    session: &SharedSession,
    input: &SyncSender<Vec<u8>>,
) -> io::Result<()> {
    let mut bytes = [0; CHUNK];
    loop {
        match byte(stream)? {
            INPUT => {
                let data = protocol::read_data(stream, &mut bytes)?;
                if input.send(data.to_vec()).is_err() {
                    break;
                }
            }
            RESIZE => {
                let size = Size::read(stream)?;
                let mut session = session.lock().unwrap_or_else(|e| e.into_inner());
                let Some(client) = session.attachments.get_mut(&id) else {
                    break;
                };
                client.size = size;
                session.resize();
            }
            _ => return Err(protocol::invalid("unknown attachment request")),
        }
    }
    Ok(())
}

fn send_output(
    stream: &mut UnixStream,
    replay: Vec<u8>,
    queue: Receiver<Output>,
    session: &SharedSession,
) -> io::Result<()> {
    stream.write_all(&[ATTACH])?;
    protocol::write_data(stream, OUTPUT, &replay)?;
    for bytes in queue {
        protocol::write_data(stream, OUTPUT, &bytes)?;
    }
    let exit = session.lock().unwrap_or_else(|e| e.into_inner()).exit;
    if let Some(code) = exit {
        stream.write_all(&[EXIT, code])?;
    }
    Ok(())
}
