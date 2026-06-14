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

use musce_proto::{Capabilities, Command, ConnectionId};

use crate::connection::{Connection, LineReader, LineWriter, Registry, serve_connection};

/// A TCP connection's read half, buffered for line framing.
pub struct TcpReader(BufReader<OwnedReadHalf>);

impl LineReader for TcpReader {
    async fn next_line(&mut self) -> io::Result<Option<String>> {
        let mut buf = String::new();
        match self.0.read_line(&mut buf).await? {
            0 => Ok(None), // EOF
            _ => {
                // Strip the line terminator (\n or \r\n) the client sent.
                let line = buf.trim_end_matches(['\r', '\n']).to_string();
                Ok(Some(line))
            }
        }
    }
}

/// A TCP connection's write half.
pub struct TcpWriter(OwnedWriteHalf);

impl LineWriter for TcpWriter {
    async fn write_line(&mut self, line: &str) -> io::Result<()> {
        self.0.write_all(line.as_bytes()).await?;
        self.0.flush().await
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
        Capabilities { color: false, line_mode_only: true, size: None }
    }

    fn split(self) -> (Self::Reader, Self::Writer) {
        let (r, w) = self.0.into_split();
        (TcpReader(BufReader::new(r)), TcpWriter(w))
    }
}

/// Bind and run the accept loop. For each connection: allocate an id, register a
/// mailbox, and spawn `serve_connection`. Returns the bound address (useful when
/// binding to port 0). The loop runs until the task is dropped.
pub async fn listen(
    addr: SocketAddr,
    inbox: Sender<Command>,
    registry: Registry,
) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    let ids = Arc::new(AtomicU64::new(0));

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

            let id = ConnectionId::next(&ids);
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            registry.lock().unwrap().insert(id, tx);

            tracing::info!(?id, %peer, "connection opened");
            tokio::spawn(serve_connection(
                id,
                Some(peer),
                TcpConnection(stream),
                inbox.clone(),
                rx,
                registry.clone(),
            ));
        }
    });

    Ok(bound)
}
