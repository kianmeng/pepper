use std::ops::Range;

use crate::{buffer::BufferHandle, buffer_position::BufferRange};

pub struct EditorEventText {
    texts_range: Range<usize>,
}
impl EditorEventText {
    pub fn as_str<'a>(&self, events: &'a EditorEventDoubleQueue) -> &'a str {
        &events.read.texts[self.texts_range.clone()]
    }
}

pub enum EditorEvent {
    Idle,
    BufferLoad {
        handle: BufferHandle,
    },
    BufferOpen {
        handle: BufferHandle,
    },
    BufferInsertText {
        handle: BufferHandle,
        range: BufferRange,
        text: EditorEventText,
    },
    BufferDeleteText {
        handle: BufferHandle,
        range: BufferRange,
    },
    BufferSave {
        handle: BufferHandle,
        new_path: bool,
    },
    BufferClose {
        handle: BufferHandle,
    },
}

// TODO: delete
#[derive(Default)]
pub struct EditorEventQueue {
    events: Vec<EditorEvent>,
    texts: String,
}

impl EditorEventQueue {
    pub fn enqueue(&mut self, event: EditorEvent) {
        self.events.push(event);
    }

    pub fn enqueue_buffer_insert(&mut self, handle: BufferHandle, range: BufferRange, text: &str) {
        let start = self.texts.len();
        self.texts.push_str(text);
        let text = EditorEventText {
            texts_range: start..self.texts.len(),
        };
        self.events.push(EditorEvent::BufferInsertText {
            handle,
            range,
            text,
        });
    }
}

// TODO: rename to EditorEventQueue
#[derive(Default)]
pub struct EditorEventDoubleQueue {
    read: EditorEventQueue,
    write: EditorEventQueue,
}

impl EditorEventDoubleQueue {
    pub fn flip(&mut self) {
        self.read.events.clear();
        self.read.texts.clear();
        std::mem::swap(&mut self.read, &mut self.write);
    }

    pub fn enqueue(&mut self, event: EditorEvent) {
        self.write.events.push(event);
    }

    pub fn enqueue_buffer_insert(&mut self, handle: BufferHandle, range: BufferRange, text: &str) {
        let start = self.write.texts.len();
        self.write.texts.push_str(text);
        let text = EditorEventText {
            texts_range: start..self.write.texts.len(),
        };
        self.write.events.push(EditorEvent::BufferInsertText {
            handle,
            range,
            text,
        });
    }

    pub fn iter<'a>(&'a self) -> impl Iterator<Item = &'a EditorEvent> {
        self.read.events.iter()
    }
}
