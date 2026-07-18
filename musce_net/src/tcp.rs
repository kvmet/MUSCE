//! Raw TCP line-mode transport: the dumb dev transport, built first to make the
//! tick loop interactive. A plain client (telnet, `nc`) talks to it in line
//! mode. It is one `Connection` impl among future ones (WebSocket, SSH); the
//! accept loop and everything above are transport-agnostic.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use crossbeam_channel::Sender;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use musce_proto::{Capabilities, Command, Delivery, Input, ServerMsg};

use crate::connection::{Connection, EventWriter, InputReader, Registry, spawn_serve};

/// A TCP connection's read half, buffered for line framing.
pub struct TcpReader(BufReader<OwnedReadHalf>);

impl InputReader for TcpReader {
    async fn next_input(&mut self) -> io::Result<Option<Input>> {
        let mut buf = String::new();
        match self.0.read_line(&mut buf).await? {
            0 => Ok(None), // EOF
            _ => {
                // Strip the line terminator (\n or \r\n) the client sent. A telnet
                // client only ever sends text, so every line is an `Input::Line`.
                let line = buf.trim_end_matches(['\r', '\n']).to_string();
                Ok(Some(Input::Line(line)))
            }
        }
    }
}

/// A TCP connection's write half. Frames each event with CRLF, which line-mode
/// clients (telnet, `nc`) expect as the terminator.
pub struct TcpWriter(OwnedWriteHalf);

impl EventWriter for TcpWriter {
    async fn write_event(&mut self, ev: &Delivery) -> io::Result<()> {
        self.0.write_all(ev.text.as_bytes()).await?;
        self.0.write_all(b"\r\n").await?;
        self.0.flush().await
    }

    async fn write_reply(&mut self, _reply: &ServerMsg) -> io::Result<()> {
        // A line-mode client never sends a query, so it never receives a reply. One
        // routed here is an upstream bug, not content to render; drop it with a
        // trace rather than dumping JSON onto a telnet session.
        tracing::debug!("dropping a structured reply bound for a line-mode connection");
        Ok(())
    }
}

/// One accepted TCP connection.
pub struct TcpConnection(tokio::net::TcpStream);

impl Connection for TcpConnection {
    type Reader = TcpReader;
    type Writer = TcpWriter;

    fn capabilities(&self) -> Capabilities {
        // A raw TCP client is line-only and of unknown color/size. SSH and
        // WebSocket will report richer capabilities.
        Capabilities {
            color: false,
            line_mode_only: true,
            size: None,
        }
    }

    fn split(self) -> (Self::Reader, Self::Writer) {
        let (r, w) = self.0.into_split();
        (TcpReader(BufReader::new(r)), TcpWriter(w))
    }
}

/// Bind and run the accept loop. Each accepted socket becomes a `TcpConnection`
/// handed to `spawn_serve`, which allocates its id (from the shared counter, so
/// ids never collide with another transport's) and drives it. Returns the bound
/// address (useful when binding to port 0). The loop runs until the task is
/// dropped.
pub async fn listen(
    addr: SocketAddr,
    inbox: Sender<Command>,
    registry: Registry,
    ids: Arc<AtomicU64>,
) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;

    tokio::spawn(async move {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                    continue;
                }
            };
            let _ = stream.set_nodelay(true);
            spawn_serve(
                TcpConnection(stream),
                Some(peer),
                inbox.clone(),
                registry.clone(),
                &ids,
            );
        }
    });

    Ok(bound)
}
