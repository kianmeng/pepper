use std::{
    collections::VecDeque,
    env, fs, io,
    os::unix::{
        ffi::OsStrExt,
        io::{AsRawFd, RawFd},
        net::{UnixListener, UnixStream},
    },
    path::Path,
    process::Child,
    time::Duration,
};

use crate::{
    application::{ApplicationConfig, ClientApplication},
    editor_utils::hash_bytes,
    platform::{BufPool, Key, KeyCode, PooledBuf, ProcessTag},
    Args,
};

fn spawn_server() {
    let mut file_actions = unsafe {
        let mut file_actions = std::mem::zeroed::<libc::posix_spawn_file_actions_t>();
        if libc::posix_spawn_file_actions_init(&mut file_actions) != 0 {
            panic!("could not init posix spawn file actions");
        }
        if libc::posix_spawn_file_actions_addclose(&mut file_actions, libc::STDIN_FILENO) != 0 {
            panic!("could not add close stdin to posix spawn file actions");
        }
        if libc::posix_spawn_file_actions_addclose(&mut file_actions, libc::STDOUT_FILENO) != 0 {
            panic!("could not add close stdout to posix spawn file actions");
        }
        file_actions
    };

    let mut attributes = unsafe {
        let mut attributes = std::mem::zeroed::<libc::posix_spawnattr_t>();
        if libc::posix_spawnattr_init(&mut attributes) != 0 {
            panic!("could not init posix spawn attributes");
        }
        if libc::posix_spawnattr_setflags(&mut attributes, libc::POSIX_SPAWN_SETPGROUP as _) != 0 {
            panic!("could not set posix spawn attributes flags");
        }
        if libc::posix_spawnattr_setpgroup(&mut attributes, 0) != 0 {
            panic!("could not set pgroup to posix spawn attributes");
        }
        attributes
    };

    let argv_owned: Vec<_> = std::env::args_os().collect();
    let mut argv = Vec::new();
    let mut args = argv_owned.iter();
    match args.next() {
        Some(arg) => argv.push(arg.as_bytes().as_ptr()),
        None => panic!("could not extract process path from argv"),
    }
    argv.push("--server\0".as_ptr());
    for arg in args {
        argv.push(arg.as_bytes().as_ptr());
    }
    argv.push(std::ptr::null());

    let mut envp_owned = Vec::new();
    for (key, value) in std::env::vars_os() {
        let mut env = key;
        env.push("=");
        env.push(&value);
        env.push("\0");
        envp_owned.push(env);
    }
    let mut envp = Vec::new();
    for var in &envp_owned {
        envp.push(var.as_bytes().as_ptr());
    }
    envp.push(std::ptr::null());

    unsafe {
        let result = libc::posix_spawnp(
            std::ptr::null_mut(),
            argv[0] as _,
            &file_actions,
            &attributes,
            argv.as_ptr() as _,
            envp.as_ptr() as _,
        );
        if result != 0 {
            panic!("could not spawn server {:?}", &argv_owned[0]);
        }

        if libc::posix_spawn_file_actions_destroy(&mut file_actions) != 0 {
            panic!("could not destroy posix spawn file actions");
        }
        if libc::posix_spawnattr_destroy(&mut attributes) != 0 {
            panic!("could not destroy posix spawn attributes");
        }
    }
}

pub(crate) fn run(
    mut config: ApplicationConfig,
    server_fn: fn(ApplicationConfig, UnixListener),
    client_fn: fn(Args, UnixStream),
) {
    if config.args.session_name.is_empty() {
        use std::fmt::Write;

        let current_dir = env::current_dir().expect("could not retrieve the current directory");
        let current_dir_bytes = current_dir.as_os_str().as_bytes();
        let current_directory_hash = hash_bytes(current_dir_bytes);

        write!(config.args.session_name, "{:x}", current_directory_hash).unwrap();
    }

    let mut session_path = String::new();
    session_path.push_str("/tmp/");
    session_path.push_str(env!("CARGO_PKG_NAME"));
    session_path.push('/');
    session_path.push_str(&config.args.session_name);

    if config.args.print_session {
        print!("{}", session_path);
        return;
    }

    let session_path = Path::new(&session_path);

    if config.args.server {
        if let Some(dir) = session_path.parent() {
            if !dir.exists() {
                let _ = fs::create_dir(dir);
            }
        }

        let _ = fs::remove_file(session_path);
        let listener = UnixListener::bind(session_path).expect("could not start unix domain socket server");

        server_fn(config, listener);
        let _ = fs::remove_file(session_path);
    } else {
        match UnixStream::connect(session_path) {
            Ok(stream) => client_fn(config.args, stream),
            Err(_) => {
                spawn_server();
                loop {
                    match UnixStream::connect(session_path) {
                        Ok(stream) => {
                            client_fn(config.args, stream);
                            break;
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(100)),
                    }
                }
            }
        }
    }
}

pub(crate) fn is_pipped(fd: RawFd) -> bool {
    unsafe { libc::isatty(fd) != true as _ }
}

pub(crate) struct Terminal {
    fd: RawFd,
    original_state: libc::termios,
}
impl Terminal {
    pub fn new() -> Self {
        let flags = libc::O_RDWR | libc::O_CLOEXEC;
        let fd = unsafe { libc::open("/dev/tty\0".as_ptr() as _, flags) };
        if fd < 0 {
            panic!("could not open terminal");
        }

        let original_state = unsafe {
            let mut original_state = std::mem::zeroed();
            libc::tcgetattr(fd, &mut original_state);
            original_state
        };

        Self { fd, original_state }
    }

    pub fn to_client_output(&self) -> ClientOutput {
        ClientOutput(self.fd)
    }

    pub fn enter_raw_mode(&self) {
        let mut next_state = self.original_state.clone();
        next_state.c_iflag &= !(libc::IGNBRK
            | libc::BRKINT
            | libc::PARMRK
            | libc::ISTRIP
            | libc::INLCR
            | libc::IGNCR
            | libc::ICRNL
            | libc::IXON);
        next_state.c_oflag &= !libc::OPOST;
        next_state.c_cflag &= !(libc::CSIZE | libc::PARENB);
        next_state.c_cflag |= libc::CS8;
        next_state.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG | libc::IEXTEN);
        next_state.c_lflag |= libc::NOFLSH;
        next_state.c_cc[libc::VMIN] = 0;
        next_state.c_cc[libc::VTIME] = 0;
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &next_state) };

        // TODO: enable kitty keyboard protocol
        // https://sw.kovidgoyal.net/kitty/keyboard-protocol/
        //write_all_bytes(self.fd, b"\x1b[>1u");
    }

    pub fn leave_raw_mode(&self) {
        // TODO: enable kitty keyboard protocol
        // https://sw.kovidgoyal.net/kitty/keyboard-protocol/
        //write_all_bytes(self.fd, b"\x1b[<u");
        unsafe { libc::tcsetattr(self.fd, libc::TCSAFLUSH, &self.original_state) };
    }

    pub fn get_size(&self) -> (u16, u16) {
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::ioctl(
                self.fd,
                libc::TIOCGWINSZ as _,
                &mut size as *mut libc::winsize,
            )
        };
        if result == -1 || size.ws_col == 0 || size.ws_row == 0 {
            panic!("could not get terminal size");
        }

        (size.ws_col as _, size.ws_row as _)
    }

    pub fn parse_keys(&self, mut buf: &[u8], keys: &mut Vec<Key>) {
        let backspace_code = self.original_state.c_cc[libc::VERASE];
        loop {
            let mut shift = false;
            let mut control = false;
            let alt = false;

            let (mut code, rest) = match buf {
                &[] => break,
                &[b, ref rest @ ..] if b == backspace_code => (KeyCode::Backspace, rest),
                &[0x1b, b'[', b'5', b'~', ref rest @ ..] => (KeyCode::PageUp, rest),
                &[0x1b, b'[', b'6', b'~', ref rest @ ..] => (KeyCode::PageDown, rest),
                &[0x1b, b'[', b'A', ref rest @ ..] => (KeyCode::Up, rest),
                &[0x1b, b'[', b'B', ref rest @ ..] => (KeyCode::Down, rest),
                &[0x1b, b'[', b'C', ref rest @ ..] => (KeyCode::Right, rest),
                &[0x1b, b'[', b'D', ref rest @ ..] => (KeyCode::Left, rest),
                &[0x1b, b'[', b'1', b'3', b'u', ref rest @ ..] => (KeyCode::Char('\n'), rest),
                &[0x1b, b'[', b'2', b'7', b'u', ref rest @ ..] => (KeyCode::Esc, rest),
                &[0x1b, b'[', b'1', b'~', ref rest @ ..]
                | &[0x1b, b'[', b'7', b'~', ref rest @ ..]
                | &[0x1b, b'[', b'H', ref rest @ ..]
                | &[0x1b, b'O', b'H', ref rest @ ..] => (KeyCode::Home, rest),
                &[0x1b, b'[', b'4', b'~', ref rest @ ..]
                | &[0x1b, b'[', b'8', b'~', ref rest @ ..]
                | &[0x1b, b'[', b'F', ref rest @ ..]
                | &[0x1b, b'O', b'F', ref rest @ ..] => (KeyCode::End, rest),
                &[0x1b, b'[', b'3', b'~', ref rest @ ..] => (KeyCode::Delete, rest),
                &[0x1b, b'[', b'9', b'u', ref rest @ ..] => (KeyCode::Char('\t'), rest),
                &[0x1b, ref rest @ ..] => (KeyCode::Esc, rest),
                &[0x8, ref rest @ ..] => (KeyCode::Backspace, rest),
                &[b'\r', ref rest @ ..] => (KeyCode::Char('\n'), rest),
                &[b'\t', ref rest @ ..] => (KeyCode::Char('\t'), rest),
                &[0x7f, ref rest @ ..] => (KeyCode::Delete, rest),
                &[b @ 0b0..=0b11111, ref rest @ ..] => {
                    control = true;
                    let byte = b | 0b01100000;
                    (KeyCode::Char(byte as _), rest)
                }
                _ => match buf.iter().position(|b| b.is_ascii()).unwrap_or(buf.len()) {
                    0 => (KeyCode::Char(buf[0] as _), &buf[1..]),
                    len => {
                        let (c, rest) = buf.split_at(len);
                        match std::str::from_utf8(c) {
                            Ok(s) => match s.chars().next() {
                                Some(c) => (KeyCode::Char(c), rest),
                                None => (KeyCode::None, rest),
                            },
                            Err(_) => (KeyCode::None, rest),
                        }
                    }
                },
            };

            if let KeyCode::Char(c) = &mut code {
                if shift {
                    *c = c.to_ascii_uppercase();
                } else {
                    shift = c.is_ascii_uppercase();
                }
            }

            let key = Key {
                code,
                shift,
                control,
                alt,
            };

            buf = rest;
            keys.push(key);
        }
    }
}
impl AsRawFd for Terminal {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        self.leave_raw_mode()
    }
}

pub(crate) fn read(fd: RawFd, buf: &mut [u8]) -> Result<usize, ()> {
    let len = unsafe { libc::read(fd, buf.as_mut_ptr() as _, buf.len()) };
    if len >= 0 {
        Ok(len as _)
    } else {
        Err(())
    }
}

pub(crate) fn write_all_bytes(fd: RawFd, mut buf: &[u8]) -> bool {
    while !buf.is_empty() {
        let len = unsafe { libc::write(fd, buf.as_ptr() as _, buf.len()) };
        if len > 0 {
            buf = &buf[len as usize..];
        } else {
            return false;
        }
    }

    true
}

pub(crate) fn read_from_connection(
    connection: &mut UnixStream,
    buf_pool: &mut BufPool,
    len: usize,
) -> Result<PooledBuf, ()> {
    use io::Read;
    let mut buf = buf_pool.acquire();
    let write = buf.write();

    loop {
        let start = write.len();
        write.resize(start + len, 0);
        match connection.read(&mut write[start..start + len]) {
            Err(error) => {
                match error.kind() {
                    io::ErrorKind::WouldBlock => write.truncate(start),
                    _ => write.clear(),
                }
                break;
            }
            Ok(len) => {
                write.truncate(start + len);
                if len == 0 {
                    break;
                }
            }
        }
    }

    if write.is_empty() {
        buf_pool.release(buf);
        Err(())
    } else {
        Ok(buf)
    }
}

pub(crate) fn write_to_connection(
    connection: &mut UnixStream,
    buf_pool: &mut BufPool,
    write_queue: &mut VecDeque<PooledBuf>,
) -> Result<(), ()> {
    use io::Write;

    loop {
        let mut buf = match write_queue.pop_front() {
            Some(buf) => buf,
            None => return Ok(()),
        };

        match connection.write(buf.as_bytes()) {
            Ok(len) => {
                buf.drain_start(len);
                if buf.as_bytes().is_empty() {
                    buf_pool.release(buf);
                } else {
                    write_queue.push_front(buf);
                }
            }
            Err(error) => match error.kind() {
                io::ErrorKind::WouldBlock => {
                    eprintln!("would block writing to connection");
                    write_queue.push_front(buf);
                    return Ok(());
                }
                _ => {
                    buf_pool.release(buf);
                    for buf in write_queue.drain(..) {
                        buf_pool.release(buf);
                    }
                    return Err(());
                }
            },
        }
    }
}

pub(crate) struct Process {
    alive: bool,
    child: Child,
    tag: ProcessTag,
    buf_len: usize,
}
impl Process {
    pub fn new(child: Child, tag: ProcessTag, buf_len: usize) -> Self {
        Self {
            alive: true,
            child,
            tag,
            buf_len,
        }
    }

    pub fn tag(&self) -> ProcessTag {
        self.tag
    }

    pub fn try_as_raw_fd(&self) -> Option<RawFd> {
        self.child.stdout.as_ref().map(|s| s.as_raw_fd())
    }

    pub fn read(&mut self, buf_pool: &mut BufPool) -> Result<Option<PooledBuf>, ()> {
        use io::Read;
        match self.child.stdout {
            Some(ref mut stdout) => {
                let mut buf = buf_pool.acquire();
                let write = buf.write_with_len(self.buf_len);
                match stdout.read(write) {
                    Ok(0) | Err(_) => {
                        buf_pool.release(buf);
                        Err(())
                    }
                    Ok(len) => {
                        write.truncate(len);
                        Ok(Some(buf))
                    }
                }
            }
            None => Ok(None),
        }
    }

    pub fn write(&mut self, buf: &[u8]) -> bool {
        use io::Write;
        match self.child.stdin {
            Some(ref mut stdin) => stdin.write_all(buf).is_ok(),
            None => true,
        }
    }

    pub fn close_input(&mut self) {
        self.child.stdin = None;
    }

    pub fn kill(&mut self) {
        if !self.alive {
            return;
        }

        self.alive = false;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.kill();
        self.alive = false;
    }
}

pub(crate) fn suspend_process<O>(
    application: &mut ClientApplication<O>,
    terminal: Option<&Terminal>,
) where
    O: io::Write,
{
    application.restore_screen();
    if let Some(terminal) = terminal {
        terminal.leave_raw_mode();
    }

    unsafe { libc::raise(libc::SIGTSTP) };

    if let Some(terminal) = terminal {
        terminal.enter_raw_mode();
    }
    application.reinit_screen();
}

pub struct ClientOutput(RawFd);
impl io::Write for ClientOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let len = unsafe { libc::write(self.0, buf.as_ptr() as _, buf.len()) };
        if len >= 0 {
            Ok(len as _)
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
