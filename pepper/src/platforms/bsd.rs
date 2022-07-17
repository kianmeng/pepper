use std::{
    collections::VecDeque,
    io,
    os::unix::{
        io::{AsRawFd, RawFd},
        net::{UnixListener, UnixStream},
    },
    time::Duration,
};

use crate::{
    application::{
        ApplicationConfig, ClientApplication, ServerApplication, CLIENT_CONNECTION_BUFFER_LEN,
        CLIENT_STDIN_BUFFER_LEN, SERVER_CONNECTION_BUFFER_LEN, SERVER_IDLE_DURATION,
    },
    client::ClientHandle,
    platform::{
        drop_request, Key, PlatformEvent, PlatformProcessHandle, PlatformRequest, PooledBuf,
    },
    Args,
};

mod unix_utils;
use unix_utils::{
    is_pipped, read, read_from_connection, run, suspend_process, write_all_bytes,
    write_to_connection, Process, Terminal,
};

const MAX_CLIENT_COUNT: usize = 20;
const MAX_PROCESS_COUNT: usize = 43;
const MAX_TRIGGERED_EVENT_COUNT: usize = 32;

pub fn try_attach_debugger() {}

pub fn main(config: ApplicationConfig) {
    run(config, run_server, run_client);
}

fn errno() -> libc::c_int {
    unsafe { *libc::__error() }
}

enum EventKind {
    Read,
    Write,
}

enum Event {
    Resize,
    FdRead(RawFd),
    FdWrite(RawFd),
}
impl Event {
    pub fn into_kevent(self, flags: u16, index: usize) -> libc::kevent {
        match self {
            Self::Resize => libc::kevent {
                ident: libc::SIGWINCH as _,
                filter: libc::EVFILT_SIGNAL,
                flags,
                fflags: 0,
                data: 0,
                udata: index as _,
            },
            Self::FdRead(fd) => libc::kevent {
                ident: fd as _,
                filter: libc::EVFILT_READ,
                flags,
                fflags: 0,
                data: 0,
                udata: index as _,
            },
            Self::FdWrite(fd) => libc::kevent {
                ident: fd as _,
                filter: libc::EVFILT_WRITE,
                flags,
                fflags: 0,
                data: 0,
                udata: index as _,
            },
        }
    }
}

struct TriggeredEvent {
    pub index: usize,
    pub data: isize,
    pub kind: EventKind,
}

struct KqueueEvents([libc::kevent; MAX_TRIGGERED_EVENT_COUNT]);
impl KqueueEvents {
    pub fn new() -> Self {
        const DEFAULT_KEVENT: libc::kevent = libc::kevent {
            ident: 0,
            filter: 0,
            flags: 0,
            fflags: 0,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        Self([DEFAULT_KEVENT; MAX_TRIGGERED_EVENT_COUNT])
    }
}

fn modify_kqueue(fd: RawFd, event: &libc::kevent) -> bool {
    unsafe { libc::kevent(fd, event as _, 1, std::ptr::null_mut(), 0, std::ptr::null()) == 0 }
}

struct Kqueue(RawFd);
impl Kqueue {
    pub fn new() -> Self {
        let fd = unsafe { libc::kqueue() };
        if fd == -1 {
            panic!("could not create kqueue, errno: {}", errno());
        }
        Self(fd)
    }

    pub fn add(&self, event: Event, index: usize, extra_flags: u16) {
        let event = event.into_kevent(libc::EV_ADD | extra_flags, index);
        if !modify_kqueue(self.0, &event) {
            panic!("could not add event, errno: {}", errno());
        }
    }

    pub fn remove(&self, event: Event) {
        let event = event.into_kevent(libc::EV_DELETE, 0);
        if !modify_kqueue(self.0, &event) {
            panic!("could not remove event, errno: {}", errno());
        }
    }

    pub fn wait<'a>(
        &self,
        events: &'a mut KqueueEvents,
        timeout: Option<Duration>,
    ) -> impl 'a + ExactSizeIterator<Item = Result<TriggeredEvent, ()>> {
        let mut timespec = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let timeout = match timeout {
            Some(duration) => {
                timespec.tv_sec = duration.as_secs() as _;
                timespec.tv_nsec = duration.subsec_nanos() as _;
                &timespec as _
            }
            None => std::ptr::null(),
        };

        let mut len = unsafe {
            libc::kevent(
                self.0,
                [].as_ptr(),
                0,
                events.0.as_mut_ptr(),
                events.0.len() as _,
                timeout,
            )
        };
        if len == -1 {
            if errno() == libc::EINTR {
                len = 0;
            } else {
                panic!("could not wait for events, errno: {}", errno());
            }
        }

        events.0[..len as usize].iter().map(|e| {
            if e.flags & libc::EV_ERROR != 0 {
                Err(())
            } else {
                let kind = match e.filter {
                    libc::EVFILT_READ | libc::EVFILT_SIGNAL => EventKind::Read,
                    libc::EVFILT_WRITE => EventKind::Write,
                    _ => unreachable!(),
                };

                Ok(TriggeredEvent {
                    index: e.udata as _,
                    data: e.data as _,
                    kind,
                })
            }
        })
    }
}
impl AsRawFd for Kqueue {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}
impl Drop for Kqueue {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

fn run_server(config: ApplicationConfig, listener: UnixListener) {
    const NONE_PROCESS: Option<Process> = None;

    let mut application = match ServerApplication::new(config) {
        Some(application) => application,
        None => return,
    };

    let mut client_connections: [Option<UnixStream>; MAX_CLIENT_COUNT] = Default::default();
    let mut client_write_queue: [VecDeque<PooledBuf>; MAX_CLIENT_COUNT] = Default::default();
    let mut processes = [NONE_PROCESS; MAX_PROCESS_COUNT];

    let mut events = Vec::new();
    let mut timeout = None;
    let mut need_redraw = false;

    const CLIENTS_START_INDEX: usize = 1;
    const CLIENTS_LAST_INDEX: usize = CLIENTS_START_INDEX + MAX_CLIENT_COUNT - 1;
    const PROCESSES_START_INDEX: usize = CLIENTS_LAST_INDEX + 1;
    const PROCESSES_LAST_INDEX: usize = PROCESSES_START_INDEX + MAX_PROCESS_COUNT - 1;

    let kqueue = Kqueue::new();
    kqueue.add(Event::FdRead(listener.as_raw_fd()), 0, 0);
    let mut kqueue_events = KqueueEvents::new();

    let _ignore_server_connection_buffer_len = SERVER_CONNECTION_BUFFER_LEN;

    loop {
        let previous_timeout = timeout;
        let kqueue_events = kqueue.wait(&mut kqueue_events, timeout);
        if kqueue_events.len() == 0 {
            match timeout {
                Some(Duration::ZERO) => timeout = Some(SERVER_IDLE_DURATION),
                Some(_) => {
                    events.push(PlatformEvent::Idle);
                    timeout = None;
                }
                None => continue,
            }
        } else {
            timeout = Some(Duration::ZERO);
        }

        for event in kqueue_events {
            let (event_index, event_data, event_kind) = match event {
                Ok(event) => (event.index, event.data, event.kind),
                Err(()) => {
                    for queue in &mut client_write_queue {
                        for buf in queue.drain(..) {
                            application.ctx.platform.buf_pool.release(buf);
                        }
                    }
                    return;
                }
            };

            match event_index {
                0 => {
                    for _ in 0..event_data {
                        match listener.accept() {
                            Ok((connection, _)) => {
                                if let Err(error) = connection.set_nonblocking(true) {
                                    panic!("could not set connection to nonblocking {}", error);
                                }

                                for (i, c) in client_connections.iter_mut().enumerate() {
                                    if c.is_none() {
                                        kqueue.add(
                                            Event::FdRead(connection.as_raw_fd()),
                                            CLIENTS_START_INDEX + i,
                                            libc::EV_CLEAR,
                                        );
                                        kqueue.add(
                                            Event::FdWrite(connection.as_raw_fd()),
                                            CLIENTS_START_INDEX + i,
                                            libc::EV_CLEAR,
                                        );
                                        *c = Some(connection);
                                        let handle = ClientHandle(i as _);
                                        events.push(PlatformEvent::ConnectionOpen { handle });
                                        break;
                                    }
                                }
                            }
                            Err(error) => panic!("could not accept connection {}", error),
                        }
                    }
                }
                CLIENTS_START_INDEX..=CLIENTS_LAST_INDEX => {
                    let index = event_index - CLIENTS_START_INDEX;
                    let handle = ClientHandle(index as _);
                    if let Some(ref mut connection) = client_connections[index] {
                        match event_kind {
                            EventKind::Read => {
                                match read_from_connection(
                                    connection,
                                    &mut application.ctx.platform.buf_pool,
                                    event_data as _,
                                ) {
                                    Ok(buf) => {
                                        events
                                            .push(PlatformEvent::ConnectionOutput { handle, buf });
                                    }
                                    Err(()) => {
                                        kqueue.remove(Event::FdRead(connection.as_raw_fd()));
                                        kqueue.remove(Event::FdWrite(connection.as_raw_fd()));
                                        client_connections[index] = None;
                                        events.push(PlatformEvent::ConnectionClose { handle });
                                    }
                                }
                            }
                            EventKind::Write => {
                                timeout = previous_timeout;

                                let result = write_to_connection(
                                    connection,
                                    &mut application.ctx.platform.buf_pool,
                                    &mut client_write_queue[index],
                                );
                                if result.is_err() {
                                    kqueue.remove(Event::FdRead(connection.as_raw_fd()));
                                    kqueue.remove(Event::FdWrite(connection.as_raw_fd()));
                                    client_connections[index] = None;
                                    events.push(PlatformEvent::ConnectionClose { handle });
                                }
                            }
                        }
                    }
                }
                PROCESSES_START_INDEX..=PROCESSES_LAST_INDEX => {
                    let index = event_index - PROCESSES_START_INDEX;
                    if let Some(ref mut process) = processes[index] {
                        let tag = process.tag();
                        match process.read(&mut application.ctx.platform.buf_pool) {
                            Ok(None) => (),
                            Ok(Some(buf)) => events.push(PlatformEvent::ProcessOutput { tag, buf }),
                            Err(()) => {
                                if let Some(fd) = process.try_as_raw_fd() {
                                    kqueue.remove(Event::FdRead(fd));
                                }
                                process.kill();
                                processes[index] = None;
                                events.push(PlatformEvent::ProcessExit { tag });
                            }
                        }
                    }
                }
                _ => unreachable!(),
            }
        }

        if events.is_empty() && !need_redraw {
            continue;
        }

        need_redraw = false;
        application.update(events.drain(..));
        let mut requests = application.ctx.platform.requests.drain();
        while let Some(request) = requests.next() {
            match request {
                PlatformRequest::Quit => {
                    for queue in &mut client_write_queue {
                        for buf in queue.drain(..) {
                            application.ctx.platform.buf_pool.release(buf);
                        }
                    }
                    for request in requests {
                        drop_request(&mut application.ctx.platform.buf_pool, request);
                    }
                    return;
                }
                PlatformRequest::Redraw => {
                    need_redraw = true;
                    timeout = Some(Duration::ZERO);
                }
                PlatformRequest::WriteToClient { handle, buf } => {
                    let index = handle.0 as usize;
                    match client_connections[index] {
                        Some(ref mut connection) => {
                            let write_queue = &mut client_write_queue[index];
                            write_queue.push_back(buf);

                            let result = write_to_connection(
                                connection,
                                &mut application.ctx.platform.buf_pool,
                                write_queue,
                            );
                            if result.is_err() {
                                kqueue.remove(Event::FdRead(connection.as_raw_fd()));
                                kqueue.remove(Event::FdWrite(connection.as_raw_fd()));
                                client_connections[index] = None;
                                events.push(PlatformEvent::ConnectionClose { handle });
                            }
                        }
                        None => application.ctx.platform.buf_pool.release(buf),
                    }
                }
                PlatformRequest::CloseClient { handle } => {
                    let index = handle.0 as usize;
                    if let Some(connection) = client_connections[index].take() {
                        kqueue.remove(Event::FdRead(connection.as_raw_fd()));
                        kqueue.remove(Event::FdWrite(connection.as_raw_fd()));
                    }
                    events.push(PlatformEvent::ConnectionClose { handle });
                }
                PlatformRequest::SpawnProcess {
                    tag,
                    mut command,
                    buf_len,
                } => {
                    let mut spawned = false;
                    for (i, p) in processes.iter_mut().enumerate() {
                        if p.is_some() {
                            continue;
                        }

                        let handle = PlatformProcessHandle(i as _);
                        if let Ok(child) = command.spawn() {
                            let process = Process::new(child, tag, buf_len);
                            if let Some(fd) = process.try_as_raw_fd() {
                                kqueue.add(Event::FdRead(fd), PROCESSES_START_INDEX + i, 0);
                            }
                            *p = Some(process);
                            events.push(PlatformEvent::ProcessSpawned { tag, handle });
                            spawned = true;
                        }
                        break;
                    }
                    if !spawned {
                        events.push(PlatformEvent::ProcessExit { tag });
                    }
                }
                PlatformRequest::WriteToProcess { handle, buf } => {
                    let index = handle.0 as usize;
                    if let Some(ref mut process) = processes[index] {
                        if !process.write(buf.as_bytes()) {
                            if let Some(fd) = process.try_as_raw_fd() {
                                kqueue.remove(Event::FdRead(fd));
                            }
                            let tag = process.tag();
                            process.kill();
                            processes[index] = None;
                            events.push(PlatformEvent::ProcessExit { tag });
                        }
                    }
                    application.ctx.platform.buf_pool.release(buf);
                }
                PlatformRequest::CloseProcessInput { handle } => {
                    if let Some(ref mut process) = processes[handle.0 as usize] {
                        process.close_input();
                    }
                }
                PlatformRequest::KillProcess { handle } => {
                    let index = handle.0 as usize;
                    if let Some(ref mut process) = processes[index] {
                        if let Some(fd) = process.try_as_raw_fd() {
                            kqueue.remove(Event::FdRead(fd));
                        }
                        let tag = process.tag();
                        process.kill();
                        processes[index] = None;
                        events.push(PlatformEvent::ProcessExit { tag });
                    }
                }
            }
        }

        if !events.is_empty() {
            timeout = Some(Duration::ZERO);
        }
    }
}

fn run_client(args: Args, mut connection: UnixStream) {
    use io::{Read, Write};

    let terminal = if args.quit {
        None
    } else {
        Some(Terminal::new())
    };

    let mut application = ClientApplication::new();
    application.output = terminal.as_ref().map(Terminal::to_client_output);

    let bytes = application.init(args);
    if connection.write_all(bytes).is_err() {
        return;
    }

    let kqueue = Kqueue::new();
    kqueue.add(Event::FdRead(connection.as_raw_fd()), 1, 0);
    if is_pipped(libc::STDIN_FILENO) {
        kqueue.add(Event::FdRead(libc::STDIN_FILENO), 3, 0);
    }

    let mut kqueue_events = KqueueEvents::new();

    if let Some(terminal) = &terminal {
        terminal.enter_raw_mode();

        kqueue.add(Event::Resize, 2, 0);

        let size = terminal.get_size();
        let (_, bytes) = application.update(Some(size), &[Key::default()], None, &[]);
        if connection.write_all(bytes).is_err() {
            return;
        }
    }

    if is_pipped(libc::STDOUT_FILENO) {
        let (_, bytes) = application.update(None, &[], Some(&[]), &[]);
        if connection.write_all(bytes).is_err() {
            return;
        }
    }

    let mut keys = Vec::new();
    let buf_capacity = CLIENT_CONNECTION_BUFFER_LEN.max(CLIENT_STDIN_BUFFER_LEN);
    let mut buf = Vec::with_capacity(buf_capacity);

    let mut select_read_set = unsafe { std::mem::zeroed() };

    'main_loop: loop {
        keys.clear();

        if let Some(terminal) = &terminal {
            unsafe {
                libc::FD_ZERO(&mut select_read_set);
                libc::FD_SET(terminal.as_raw_fd(), &mut select_read_set);
                libc::FD_SET(kqueue.as_raw_fd(), &mut select_read_set);

                let result = libc::select(
                    terminal.as_raw_fd().max(kqueue.as_raw_fd()) + 1,
                    &mut select_read_set,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                if result < 0 {
                    break;
                }

                if libc::FD_ISSET(terminal.as_raw_fd(), &select_read_set) {
                    buf.resize(buf_capacity, 0);
                    match read(terminal.as_raw_fd(), &mut buf) {
                        Ok(0) | Err(()) => break,
                        Ok(len) => terminal.parse_keys(&buf[..len], &mut keys),
                    }

                    let (suspend, bytes) = application.update(None, &keys, None, &[]);
                    if connection.write_all(bytes).is_err() {
                        break;
                    }
                    if suspend {
                        suspend_process(&mut application, Some(terminal));
                    }

                    if result == 1 {
                        continue;
                    }
                }
            }
        }

        for event in kqueue.wait(&mut kqueue_events, Some(Duration::ZERO)) {
            let mut resize = None;
            let mut stdin_bytes = None;
            let mut server_bytes = &[][..];

            match event {
                Ok(TriggeredEvent { index: 1, data, .. }) => {
                    buf.resize(data as _, 0);
                    match connection.read(&mut buf) {
                        Ok(0) | Err(_) => break 'main_loop,
                        Ok(len) => server_bytes = &buf[..len],
                    }
                }
                Ok(TriggeredEvent { index: 2, .. }) => {
                    resize = terminal.as_ref().map(Terminal::get_size);
                }
                Ok(TriggeredEvent { index: 3, data, .. }) => {
                    buf.resize(data as _, 0);
                    match read(libc::STDIN_FILENO, &mut buf) {
                        Ok(0) | Err(()) => {
                            kqueue.remove(Event::FdRead(libc::STDIN_FILENO));
                            stdin_bytes = Some(&[][..]);
                        }
                        Ok(len) => stdin_bytes = Some(&buf[..len]),
                    }
                }
                Ok(_) => unreachable!(),
                Err(()) => break 'main_loop,
            }

            let (suspend, bytes) = application.update(resize, &keys, stdin_bytes, server_bytes);
            if connection.write_all(bytes).is_err() {
                break;
            }
            if suspend {
                suspend_process(&mut application, terminal.as_ref());
            }
        }
    }

    if is_pipped(libc::STDOUT_FILENO) {
        let bytes = application.get_stdout_bytes();
        write_all_bytes(libc::STDOUT_FILENO, bytes);
    }

    drop(terminal);
    drop(application);
}
