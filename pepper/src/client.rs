use std::fmt;

use crate::{
    buffer::{char_display_len, BufferHandle, BufferProperties},
    buffer_position::{BufferPosition, BufferPositionIndex},
    buffer_view::{BufferViewCollection, BufferViewHandle},
    editor::Editor,
    editor_utils::ResidualStrBytes,
    navigation_history::{NavigationHistory, NavigationMovement},
    serialization::{DeserializeError, Deserializer, Serialize, Serializer},
};

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ClientHandle(u8);

impl ClientHandle {
    pub fn into_index(self) -> usize {
        self.0 as _
    }

    pub fn from_index(index: usize) -> Option<ClientHandle> {
        if index <= u8::MAX as usize {
            Some(ClientHandle(index as _))
        } else {
            None
        }
    }
}

impl<'de> Serialize<'de> for ClientHandle {
    fn serialize<S>(&self, serializer: &mut S)
    where
        S: Serializer,
    {
        self.0.serialize(serializer);
    }

    fn deserialize<D>(deserializer: &mut D) -> Result<Self, DeserializeError>
    where
        D: Deserializer<'de>,
    {
        Ok(Self(u8::deserialize(deserializer)?))
    }
}

pub enum ViewAnchor {
    Top,
    Center,
    Bottom,
}

pub struct Client {
    active: bool,
    handle: ClientHandle,

    pub viewport_size: (u16, u16),
    pub(crate) navigation_history: NavigationHistory,

    pub scroll_offset: BufferPosition,
    scroll: BufferPositionIndex,

    buffer_view_handle: Option<BufferViewHandle>,
    stdin_buffer_handle: Option<BufferHandle>,
    stdin_residual_bytes: ResidualStrBytes,
}

impl Client {
    pub(crate) fn new() -> Self {
        Self {
            active: false,
            handle: ClientHandle(0),

            viewport_size: (0, 0),
            navigation_history: NavigationHistory::default(),

            scroll_offset: BufferPosition::zero(),
            scroll: 0,

            buffer_view_handle: None,
            stdin_buffer_handle: None,
            stdin_residual_bytes: ResidualStrBytes::default(),
        }
    }

    fn dispose(&mut self) {
        self.active = false;

        self.viewport_size = (0, 0);
        self.navigation_history.clear();

        self.scroll_offset = BufferPosition::zero();
        self.scroll = 0;

        self.buffer_view_handle = None;
        self.stdin_buffer_handle = None;
        self.stdin_residual_bytes = ResidualStrBytes::default();
    }

    pub fn handle(&self) -> ClientHandle {
        self.handle
    }

    pub fn buffer_view_handle(&self) -> Option<BufferViewHandle> {
        self.buffer_view_handle
    }

    pub fn stdin_buffer_handle(&self) -> Option<BufferHandle> {
        self.stdin_buffer_handle
    }

    pub fn set_buffer_view_handle(
        &mut self,
        handle: Option<BufferViewHandle>,
        buffer_views: &BufferViewCollection,
    ) {
        NavigationHistory::save_snapshot(self, buffer_views);
        self.set_buffer_view_handle_no_history(handle);
    }

    pub fn has_ui(&self) -> bool {
        self.viewport_size.0 != 0 && self.viewport_size.1 != 0
    }

    pub fn set_view_anchor(&mut self, editor: &Editor, anchor: ViewAnchor) {
        if !self.has_ui() {
            return;
        }

        let height = self.viewport_size.1.saturating_sub(1) as usize;
        let height_offset = match anchor {
            ViewAnchor::Top => 0,
            ViewAnchor::Center => height / 2,
            ViewAnchor::Bottom => height,
        };

        let main_cursor_height = self.find_main_cursor_height(editor);
        self.scroll = (main_cursor_height + height_offset) as _;
    }

    pub(crate) fn set_buffer_view_handle_no_history(&mut self, handle: Option<BufferViewHandle>) {
        self.buffer_view_handle = handle;
    }

    pub(crate) fn update_view(&mut self, editor: &Editor, margin_bottom: usize) {
        if !self.has_ui() {
            return;
        }

        let height = self.viewport_size.1.saturating_sub(1) as usize;
        let height = height.saturating_sub(margin_bottom);
        let half_height = height / 2;
        let quarter_height = half_height / 2;

        let main_cursor_height = self.find_main_cursor_height(editor);

        let scroll = self.scroll as usize;
        if main_cursor_height < scroll.saturating_sub(quarter_height) {
            self.scroll = main_cursor_height.saturating_sub(half_height) as _;
        } else if main_cursor_height < scroll {
            self.scroll = main_cursor_height as _;
        } else if main_cursor_height >= scroll + height + quarter_height {
            self.scroll = (main_cursor_height + 1 - half_height) as _;
        } else if main_cursor_height >= scroll + height {
            self.scroll = (main_cursor_height + 1 - height) as _;
        }
    }

    pub(crate) fn on_stdin_input(&mut self, editor: &mut Editor, bytes: &[u8]) {
        let mut buf = Default::default();
        let texts = self.stdin_residual_bytes.receive_bytes(&mut buf, bytes);

        let buffer_handle = match self.stdin_buffer_handle() {
            Some(handle) => handle,
            None => {
                use fmt::Write;

                let buffer = editor.buffers.add_new();

                let mut path = editor.string_pool.acquire_with("pipe.");
                let _ = write!(path, "{}", self.handle().into_index());
                buffer.path.clear();
                buffer.path.push(&path);
                editor.string_pool.release(path);

                buffer.properties = BufferProperties::text();
                buffer.properties.is_file = false;

                let buffer_view_handle =
                    editor.buffer_views.add_new(self.handle(), buffer.handle());
                self.set_buffer_view_handle(Some(buffer_view_handle), &editor.buffer_views);

                self.stdin_buffer_handle = Some(buffer.handle());
                buffer.handle()
            }
        };

        let buffer = editor.buffers.get_mut(buffer_handle);
        for text in texts {
            let position = buffer.content().end();
            buffer.insert_text(
                &mut editor.word_database,
                position,
                text,
                &mut editor.events,
            );
        }
    }

    pub(crate) fn on_buffer_close(&mut self, editor: &mut Editor, buffer_handle: BufferHandle) {
        self.navigation_history
            .remove_snapshots_with_buffer_handle(buffer_handle);

        if let Some(handle) = self.buffer_view_handle {
            let buffer_view = editor.buffer_views.get(handle);
            if buffer_view.buffer_handle == buffer_handle {
                self.buffer_view_handle = None;
                NavigationHistory::move_in_history(self, editor, NavigationMovement::Backward);
                NavigationHistory::move_in_history(self, editor, NavigationMovement::Forward);
            }
        }

        if self.stdin_buffer_handle == Some(buffer_handle) {
            self.stdin_buffer_handle = None;
        }
    }

    // TODO: cache cumulative display lengths
    fn find_main_cursor_height(&mut self, editor: &Editor) -> usize {
        let buffer_view_handle = match self.buffer_view_handle() {
            Some(handle) => handle,
            None => return 0,
        };

        let tab_size = editor.config.tab_size.get() as usize;
        let width = self.viewport_size.0 as usize;

        let buffer_view = editor.buffer_views.get(buffer_view_handle);
        let position = buffer_view.cursors.main_cursor().position;
        let lines = editor
            .buffers
            .get(buffer_view.buffer_handle)
            .content()
            .lines();

        let mut position_height = 0;
        for line in &lines[..position.line_index as usize] {
            position_height += 1 + line.display_len().total_len(tab_size) / width;
        }
        let cursor_line = lines[position.line_index as usize].as_str();
        position_height += find_line_height(
            &cursor_line[..position.column_byte_index as usize],
            width,
            tab_size,
        );

        position_height
    }
}

#[derive(Default)]
pub struct ClientManager {
    focused_client: Option<ClientHandle>,
    previous_focused_client: Option<ClientHandle>,
    clients: Vec<Client>,
}

impl ClientManager {
    pub fn focused_client(&self) -> Option<ClientHandle> {
        self.focused_client
    }

    pub fn previous_focused_client(&self) -> Option<ClientHandle> {
        self.previous_focused_client
    }

    pub fn focus_client(&mut self, handle: ClientHandle) -> bool {
        let client = self.get(handle);
        if !client.has_ui() {
            return false;
        }

        let changed = Some(handle) != self.focused_client;
        if changed {
            self.previous_focused_client = self.focused_client;
        }
        self.focused_client = Some(handle);
        changed
    }

    pub fn get(&self, handle: ClientHandle) -> &Client {
        &self.clients[handle.into_index()]
    }

    pub fn get_mut(&mut self, handle: ClientHandle) -> &mut Client {
        &mut self.clients[handle.into_index()]
    }

    pub fn iter(&self) -> impl Clone + Iterator<Item = &Client> {
        self.clients.iter().filter(|c| c.active)
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Client> {
        self.clients.iter_mut().filter(|c| c.active)
    }

    pub(crate) fn on_client_joined(&mut self, handle: ClientHandle) {
        let min_len = handle.into_index() + 1;
        if min_len > self.clients.len() {
            self.clients.resize_with(min_len, Client::new);
        }

        let client = &mut self.clients[handle.into_index()];
        client.active = true;
        client.handle = handle;
    }

    pub(crate) fn on_client_left(&mut self, handle: ClientHandle) {
        self.clients[handle.into_index()].dispose();
        if self.focused_client == Some(handle) {
            self.focused_client = None;
        }
    }
}

/*
fn find_wrapped_line_start_index(
    line: &str,
    viewport_width: usize,
    tab_size: usize,
    column_byte_index: usize,
) -> usize {
    let mut x = 0;
    let mut last_line_start = 0;
    for (i, c) in line.char_indices() {
        if i == column_byte_index {
            break;
        }
        match c {
            '\t' => x += tab_size,
            _ => x += char_display_len(c) as usize,
        }
        if x >= viewport_width {
            x -= viewport_width;
            last_line_start = i + c.len_utf8();
        }
    }
    last_line_start
}
*/

fn find_line_height(line: &str, viewport_width: usize, tab_size: usize) -> usize {
    let mut x = 0;
    let mut height = 1;
    for c in line.chars() {
        match c {
            '\t' => x += tab_size,
            _ => x += char_display_len(c) as usize,
        }
        if x >= viewport_width {
            x -= viewport_width;
            height += 1;
        }
    }
    height
}

#[cfg(test)]
mod tests {
    use super::*;

    /*
        #[test]
        fn find_wrapped_line_start_index_test() {
            let f = |s, column_index| find_wrapped_line_start_index(s, 4, 2, column_index);

            assert_eq!(0, f("", 0));
            assert_eq!(0, f("abc", 2));
            assert_eq!(0, f("abc", 3));
            assert_eq!(0, f("abcd", 3));
            assert_eq!(4, f("abcd", 4));
            assert_eq!(4, f("abc\t", 4));
            assert_eq!(4, f("abcdef", 4));
            assert_eq!(4, f("abcdef", 5));
            assert_eq!(4, f("abcdef", 6));
            assert_eq!(4, f("abcdefghij", 6));
        }
    */

    #[test]
    fn find_line_height_test() {
        let f = |s| find_line_height(s, 4, 2);

        assert_eq!(1, f(""));
        assert_eq!(1, f("abc"));
        assert_eq!(2, f("abcd"));
        assert_eq!(2, f("abcdefg"));
        assert_eq!(3, f("abcdefgh"));
    }
}

