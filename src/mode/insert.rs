use crate::{
    buffer::TextRef,
    buffer_position::BufferOffset,
    buffer_view::MovementKind,
    editor::KeysIterator,
    event::Key,
    mode::{Mode, ModeContext, ModeOperation},
};

pub fn on_enter(_ctx: &mut ModeContext) {}

pub fn on_event(ctx: &mut ModeContext, keys: &mut KeysIterator) -> ModeOperation {
    let handle = if let Some(handle) = ctx.current_buffer_view_handle {
        handle
    } else {
        return ModeOperation::EnterMode(Mode::Normal);
    };

    match keys.next() {
        Key::Esc | Key::Ctrl('c') => {
            ctx.buffer_views.get_mut(handle).commit_edits(ctx.buffers);
            return ModeOperation::EnterMode(Mode::Normal);
        }
        Key::Tab => ctx
            .buffer_views
            .insert_text(ctx.buffers, handle, TextRef::Char('\t')),
        Key::Ctrl('m') => ctx
            .buffer_views
            .insert_text(ctx.buffers, handle, TextRef::Char('\n')),
        Key::Char(c) => ctx
            .buffer_views
            .insert_text(ctx.buffers, handle, TextRef::Char(c)),
        Key::Ctrl('h') => {
            ctx.buffer_views.get_mut(handle).move_cursors(
                ctx.buffers,
                ctx.operations,
                BufferOffset::line_col(0, -1),
                MovementKind::PositionOnly,
            );
            ctx.buffer_views.remove_in_selection(ctx.buffers, handle);
        }
        Key::Delete => {
            ctx.buffer_views.get_mut(handle).move_cursors(
                ctx.buffers,
                ctx.operations,
                BufferOffset::line_col(0, 1),
                MovementKind::PositionOnly,
            );
            ctx.buffer_views.remove_in_selection(ctx.buffers, handle);
        }
        _ => (),
    }

    ModeOperation::None
}
