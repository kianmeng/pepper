use std::{
    io::{self, Read, Write},
    net::Shutdown,
    path::Path,
};

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};

use crate::{
    client_event::{
        ClientEvent, ClientEventDeserializeResult, ClientEventDeserializer, ClientEventSerializer,
    },
    editor::EditorLoop,
    event_manager::EventRegistry,
};

struct ReadBuf {
    buf: Vec<u8>,
    len: usize,
}

impl ReadBuf {
    pub fn new() -> Self {
        let mut buf = Vec::with_capacity(2 * 1024);
        buf.resize(buf.capacity(), 0);
        Self { buf, len: 0 }
    }

    pub fn read_from<R>(&mut self, mut reader: R) -> io::Result<&[u8]>
    where
        R: Read,
    {
        self.len = 0;
        loop {
            match reader.read(&mut self.buf[self.len..]) {
                Ok(len) => {
                    self.len += len;
                    if self.len < self.buf.len() {
                        break;
                    }
                    self.buf.resize(self.buf.len() * 2, 0);
                }
                Err(e) => match e.kind() {
                    io::ErrorKind::WouldBlock => break,
                    _ => return Err(e),
                },
            }
        }

        Ok(&self.buf[..self.len])
    }
}

pub struct ConnectionWithClient(UnixStream);

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ConnectionWithClientHandle(usize);
impl ConnectionWithClientHandle {
    pub fn from_index(index: usize) -> Self {
        Self(index)
    }

    pub fn into_index(self) -> usize {
        self.0
    }
}

pub struct ConnectionWithClientCollection {
    listener: UnixListener,
    connections: Vec<Option<ConnectionWithClient>>,
    closed_connection_indexes: Vec<usize>,
    read_buf: ReadBuf,
}

impl ConnectionWithClientCollection {
    pub fn listen<P>(path: P) -> io::Result<Self>
    where
        P: AsRef<Path>,
    {
        let listener = UnixListener::bind(path)?;
        listener.set_nonblocking(true)?;

        Ok(Self {
            listener,
            connections: Vec::new(),
            closed_connection_indexes: Vec::new(),
            read_buf: ReadBuf::new(),
        })
    }

    pub fn register_listener(&self, event_registry: &EventRegistry) -> io::Result<()> {
        event_registry.register_listener(&self.listener)
    }

    pub fn listen_next_listener_event(&self, event_registry: &EventRegistry) -> io::Result<()> {
        event_registry.listen_next_listener_event(&self.listener)
    }

    pub fn accept_connection(
        &mut self,
        event_registry: &EventRegistry,
    ) -> io::Result<ConnectionWithClientHandle> {
        let (stream, _) = self.listener.accept()?;
        stream.set_nonblocking(true)?;
        let connection = ConnectionWithClient(stream);

        for (i, slot) in self.connections.iter_mut().enumerate() {
            if slot.is_none() {
                let handle = ConnectionWithClientHandle(i);
                event_registry.register_stream(&connection.0, handle.into())?;
                *slot = Some(connection);
                return Ok(handle);
            }
        }

        let handle = ConnectionWithClientHandle(self.connections.len());
        event_registry.register_stream(&connection.0, handle.into())?;
        self.connections.push(Some(connection));
        Ok(handle)
    }

    pub fn listen_next_connection_event(
        &self,
        handle: ConnectionWithClientHandle,
        event_registry: &EventRegistry,
    ) -> io::Result<()> {
        if let Some(connection) = &self.connections[handle.0] {
            event_registry.listen_next_stream_event(&connection.0, handle.into())?;
        }

        Ok(())
    }

    pub fn close_connection(&mut self, handle: ConnectionWithClientHandle) {
        if let Some(connection) = &self.connections[handle.0] {
            let _ = connection.0.shutdown(Shutdown::Both);
            self.closed_connection_indexes.push(handle.0);
        }
    }

    pub fn close_all_connections(&mut self) {
        for connection in self.connections.iter().flatten() {
            let _ = connection.0.shutdown(Shutdown::Both);
        }
    }

    pub fn unregister_closed_connections(
        &mut self,
        event_registry: &EventRegistry,
    ) -> io::Result<()> {
        for i in self.closed_connection_indexes.drain(..) {
            if let Some(connection) = self.connections[i].take() {
                event_registry.unregister_stream(&connection.0)?;
            }
        }

        Ok(())
    }

    pub fn send_serialized_display(&mut self, handle: ConnectionWithClientHandle, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let stream = match &mut self.connections[handle.0] {
            Some(connection) => &mut connection.0,
            None => return,
        };

        if let Err(_) = stream.write_all(bytes).and_then(|_| stream.flush()) {
            self.close_connection(handle);
        }
    }

    pub fn receive_events<F>(
        &mut self,
        handle: ConnectionWithClientHandle,
        mut func: F,
    ) -> io::Result<EditorLoop>
    where
        F: FnMut(ClientEvent) -> EditorLoop,
    {
        let connection = match &mut self.connections[handle.0] {
            Some(connection) => connection,
            None => return Ok(EditorLoop::Quit),
        };

        let bytes = self.read_buf.read_from(&mut connection.0)?;
        let mut last_editor_loop = EditorLoop::Quit;
        let mut deserializer = ClientEventDeserializer::from_slice(bytes);

        loop {
            match deserializer.deserialize_next() {
                ClientEventDeserializeResult::Some(event) => {
                    last_editor_loop = func(event);
                    if last_editor_loop.is_quit() {
                        break;
                    }
                }
                ClientEventDeserializeResult::None => break,
                ClientEventDeserializeResult::Error => {
                    return Err(io::Error::from(io::ErrorKind::Other))
                }
            }
        }

        Ok(last_editor_loop)
    }
}

pub struct ConnectionWithServer {
    stream: UnixStream,
    read_buf: ReadBuf,
}

impl ConnectionWithServer {
    pub fn connect<P>(path: P) -> io::Result<Self>
    where
        P: AsRef<Path>,
    {
        let stream = UnixStream::connect(path)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            read_buf: ReadBuf::new(),
        })
    }

    pub fn close(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    pub fn register_connection(&self, event_registry: &EventRegistry) -> io::Result<()> {
        event_registry.register_stream(
            &self.stream,
            ConnectionWithClientHandle::from_index(0).into(),
        )
    }

    pub fn listen_next_event(&self, event_registry: &EventRegistry) -> io::Result<()> {
        event_registry.listen_next_stream_event(
            &self.stream,
            ConnectionWithClientHandle::from_index(0).into(),
        )
    }

    pub fn send_serialized_events(
        &mut self,
        serializer: &mut ClientEventSerializer,
    ) -> io::Result<()> {
        let bytes = serializer.bytes();
        if bytes.is_empty() {
            return Ok(());
        }

        let result = self
            .stream
            .write_all(bytes)
            .and_then(|_| self.stream.flush());

        serializer.clear();
        result
    }

    pub fn receive_display(&mut self) -> io::Result<&[u8]> {
        self.read_buf.read_from(&mut self.stream)
    }
}
