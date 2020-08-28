use std::{
    fmt,
    fs::File,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
};

use crate::{
    buffer::{Buffer, BufferContent, TextRef},
    buffer_view::BufferView,
    config::ParseConfigError,
    connection::{ConnectionWithClientHandle, TargetClient},
    editor_operation::{EditorOperation, StatusMessageKind},
    keymap::ParseKeyMapError,
    mode::Mode,
    pattern::Pattern,
    script::{ScriptContext, ScriptEngine, ScriptError, ScriptResult, ScriptStr},
    theme::ParseThemeError,
};

pub struct QuitError;
impl fmt::Display for QuitError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("could not quit now")
    }
}

pub fn bind_all<'a>(scripts: &'a mut ScriptEngine) -> ScriptResult<()> {
    macro_rules! register_all {
        ($($func:ident,)*) => {
            $(scripts.register_ctx_function(stringify!($func), bindings::$func)?;)*
        }
    }

    register_all! {
        target,
        quit, open, close, save, save_all,
        selection, replace, print, pipe,
        config, syntax_extension, syntax_rule, theme,
        mapn, maps, mapi,
    };

    Ok(())
}

mod bindings {
    use super::*;

    pub fn target(ctx: &mut ScriptContext, target: Option<usize>) -> ScriptResult<()> {
        match target {
            Some(index) => {
                let client_handle = ConnectionWithClientHandle::from_index(index);
                ctx.client_target_map
                    .map(ctx.target_client, TargetClient::Remote(client_handle));
            }
            None => ctx
                .client_target_map
                .map(ctx.target_client, TargetClient::Local),
        }
        Ok(())
    }

    pub fn quit(ctx: &mut ScriptContext, _: ()) -> ScriptResult<()> {
        *ctx.quit = true;
        Err(ScriptError::from(QuitError))
    }

    pub fn open(ctx: &mut ScriptContext, path: ScriptStr) -> ScriptResult<()> {
        let path = Path::new(path.to_str()?);
        helper::new_buffer_from_file(ctx, path).map_err(ScriptError::from)?;
        Ok(())
    }

    pub fn close(ctx: &mut ScriptContext, _: ()) -> ScriptResult<()> {
        if let Some(handle) = ctx
            .current_buffer_view_handle
            .take()
            .map(|h| ctx.buffer_views.get(&h).buffer_handle)
        {
            for view in ctx.buffer_views.iter() {
                if view.buffer_handle == handle {
                    ctx.operations
                        .serialize(view.target_client, &EditorOperation::Buffer(""));
                    ctx.operations
                        .serialize(view.target_client, &EditorOperation::Path(Path::new("")));
                }
            }
            ctx.buffer_views
                .remove_where(|view| view.buffer_handle == handle);
        }

        Ok(())
    }

    pub fn save(ctx: &mut ScriptContext, path: Option<ScriptStr>) -> ScriptResult<()> {
        let view_handle = match ctx.current_buffer_view_handle.as_ref() {
            Some(handle) => handle,
            None => return Err(ScriptError::from("no buffer opened")),
        };

        let buffer_handle = ctx.buffer_views.get(view_handle).buffer_handle;
        let buffer = match ctx.buffers.get_mut(buffer_handle) {
            Some(buffer) => buffer,
            None => return Err(ScriptError::from("no buffer opened")),
        };

        match path {
            Some(path) => {
                let path = Path::new(path.to_str()?);
                helper::write_buffer_to_file(buffer, path).map_err(ScriptError::from)?;
                for view in ctx.buffer_views.iter() {
                    if view.buffer_handle == buffer_handle {
                        ctx.operations
                            .serialize(view.target_client, &EditorOperation::Path(path));
                    }
                }
                buffer.path.clear();
                buffer.path.push(path);
                Ok(())
            }
            None => {
                if !buffer.path.as_os_str().is_empty() {
                    Err(ScriptError::from("buffer has no path"))
                } else {
                    helper::write_buffer_to_file(buffer, &buffer.path).map_err(ScriptError::from)
                }
            }
        }
    }

    pub fn save_all(ctx: &mut ScriptContext, _: ()) -> ScriptResult<()> {
        for buffer in ctx.buffers.iter() {
            if !buffer.path.as_os_str().is_empty() {
                helper::write_buffer_to_file(buffer, &buffer.path).map_err(ScriptError::from)?;
            }
        }

        Ok(())
    }

    pub fn selection(ctx: &mut ScriptContext, _: ()) -> ScriptResult<String> {
        let mut selection = String::new();
        if let Some(buffer_view) = ctx
            .current_buffer_view_handle
            .as_ref()
            .map(|h| ctx.buffer_views.get(h))
        {
            buffer_view.get_selection_text(ctx.buffers, &mut selection);
        }

        Ok(selection)
    }

    pub fn replace(ctx: &mut ScriptContext, text: ScriptStr) -> ScriptResult<()> {
        if let Some(handle) = ctx.current_buffer_view_handle {
            let text = TextRef::Str(text.to_str()?);
            ctx.buffer_views
                .delete_in_selection(ctx.buffers, ctx.operations, handle);
            ctx.buffer_views
                .insert_text(ctx.buffers, ctx.operations, handle, text);
        }
        Ok(())
    }

    pub fn print(ctx: &mut ScriptContext, message: ScriptStr) -> ScriptResult<()> {
        let message = message.to_str()?;
        println!("printing: {}", message);
        ctx.operations.serialize(
            TargetClient::All,
            &EditorOperation::StatusMessage(StatusMessageKind::Info, message),
        );
        Ok(())
    }

    pub fn pipe(
        _ctx: &mut ScriptContext,
        (name, args, input): (ScriptStr, Vec<ScriptStr>, Option<ScriptStr>),
    ) -> ScriptResult<String> {
        let mut command = Command::new(name.to_str()?);
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        for arg in args {
            command.arg(arg.to_str()?);
        }

        let mut child = command.spawn().map_err(ScriptError::from)?;
        if let Some((stdin, input)) = child.stdin.as_mut().zip(input) {
            let _ = stdin.write_all(input.as_bytes());
        }
        child.stdin = None;

        let child_output = child.wait_with_output().map_err(ScriptError::from)?;
        if child_output.status.success() {
            let child_output = String::from_utf8_lossy(&child_output.stdout[..]);
            Ok(child_output.into_owned())
        } else {
            let child_output = String::from_utf8_lossy(&child_output.stdout[..]);
            Err(ScriptError::from(child_output.into_owned()))
        }
    }

    pub fn config(
        ctx: &mut ScriptContext,
        (name, value): (ScriptStr, ScriptStr),
    ) -> ScriptResult<()> {
        let name = name.to_str()?;
        let value = value.to_str()?;

        let mut values = ctx.config.values.clone();
        if let Err(e) = values.parse_and_set(name, value) {
            let message = match e {
                ParseConfigError::ConfigNotFound => helper::parsing_error(e, name, 0),
                ParseConfigError::ParseError(e) => helper::parsing_error(e, value, 0),
            };
            return Err(ScriptError::from(message));
        }

        ctx.operations
            .serialize_config_values(TargetClient::All, &values);
        Ok(())
    }

    pub fn syntax_extension(
        ctx: &mut ScriptContext,
        (main_extension, other_extension): (ScriptStr, ScriptStr),
    ) -> ScriptResult<()> {
        let main_extension = main_extension.to_str()?;
        let other_extension = other_extension.to_str()?;
        ctx.operations.serialize(
            TargetClient::All,
            &EditorOperation::SyntaxExtension(main_extension, other_extension),
        );
        Ok(())
    }

    pub fn syntax_rule(
        ctx: &mut ScriptContext,
        (main_extension, token_kind, pattern): (ScriptStr, ScriptStr, ScriptStr),
    ) -> ScriptResult<()> {
        let main_extension = main_extension.to_str()?;
        let token_kind = token_kind.to_str()?;
        let pattern = pattern.to_str()?;

        let token_kind = token_kind.parse().map_err(ScriptError::from)?;
        let pattern = Pattern::new(pattern).map_err(|e| {
            let message = helper::parsing_error(e, pattern, 0);
            ScriptError::from(message)
        })?;

        ctx.operations.serialize_syntax_rule(
            TargetClient::All,
            main_extension,
            token_kind,
            &pattern,
        );
        Ok(())
    }

    pub fn theme(
        ctx: &mut ScriptContext,
        (name, color): (ScriptStr, ScriptStr),
    ) -> ScriptResult<()> {
        let name = name.to_str()?;
        let color = color.to_str()?;

        let mut theme = ctx.config.theme.clone();
        if let Err(e) = theme.parse_and_set(name, color) {
            let context = format!("{} {}", name, color);
            let error_index = match e {
                ParseThemeError::ColorNotFound => 0,
                _ => context.len(),
            };
            let message = helper::parsing_error(e, &context[..], error_index);
            return Err(ScriptError::from(message));
        }

        ctx.operations.serialize_theme(TargetClient::All, &theme);
        Ok(())
    }

    pub fn mapn(ctx: &mut ScriptContext, (from, to): (ScriptStr, ScriptStr)) -> ScriptResult<()> {
        map_mode(ctx, Mode::Normal, from, to)
    }

    pub fn maps(ctx: &mut ScriptContext, (from, to): (ScriptStr, ScriptStr)) -> ScriptResult<()> {
        map_mode(ctx, Mode::Select, from, to)
    }

    pub fn mapi(ctx: &mut ScriptContext, (from, to): (ScriptStr, ScriptStr)) -> ScriptResult<()> {
        map_mode(ctx, Mode::Insert, from, to)
    }

    fn map_mode(
        ctx: &mut ScriptContext,
        mode: Mode,
        from: ScriptStr,
        to: ScriptStr,
    ) -> ScriptResult<()> {
        let from = from.to_str()?;
        let to = to.to_str()?;

        match ctx.keymaps.parse_map(mode.discriminant(), from, to) {
            Ok(()) => Ok(()),
            Err(ParseKeyMapError::From(e)) => {
                let message = helper::parsing_error(e.error, from, e.index);
                Err(ScriptError::from(message))
            }
            Err(ParseKeyMapError::To(e)) => {
                let message = helper::parsing_error(e.error, to, e.index);
                Err(ScriptError::from(message))
            }
        }
    }
}

mod helper {
    use super::*;

    pub fn parsing_error<T>(message: T, text: &str, error_index: usize) -> String
    where
        T: fmt::Display,
    {
        let (before, after) = text.split_at(error_index);
        match (before.len(), after.len()) {
            (0, 0) => format!("{} at ''", message),
            (_, 0) => format!("{} at '{}' <- here", message, before),
            (0, _) => format!("{} at here -> '{}'", message, after),
            (_, _) => format!("{} at '{}' <- here '{}'", message, before, after),
        }
    }

    pub fn new_buffer_from_content(ctx: &mut ScriptContext, path: &Path, content: BufferContent) {
        ctx.operations.serialize_buffer(ctx.target_client, &content);
        ctx.operations
            .serialize(ctx.target_client, &EditorOperation::Path(path));

        let buffer_handle = ctx.buffers.add(Buffer::new(path.into(), content));
        let buffer_view = BufferView::new(ctx.target_client, buffer_handle);
        let buffer_view_handle = ctx.buffer_views.add(buffer_view);
        *ctx.current_buffer_view_handle = Some(buffer_view_handle);
    }

    pub fn new_buffer_from_file(ctx: &mut ScriptContext, path: &Path) -> Result<(), String> {
        if let Some(buffer_handle) = ctx.buffers.find_with_path(path) {
            let mut iter = ctx
                .buffer_views
                .iter_with_handles()
                .filter_map(|(handle, view)| {
                    if view.buffer_handle == buffer_handle
                        && view.target_client == ctx.target_client
                    {
                        Some((handle, view))
                    } else {
                        None
                    }
                });

            let view = match iter.next() {
                Some((handle, view)) => {
                    *ctx.current_buffer_view_handle = Some(handle);
                    view
                }
                None => {
                    drop(iter);
                    let view = BufferView::new(ctx.target_client, buffer_handle);
                    let view_handle = ctx.buffer_views.add(view);
                    let view = ctx.buffer_views.get(&view_handle);
                    *ctx.current_buffer_view_handle = Some(view_handle);
                    view
                }
            };

            ctx.operations.serialize_buffer(
                ctx.target_client,
                &ctx.buffers.get(buffer_handle).unwrap().content,
            );
            ctx.operations
                .serialize(ctx.target_client, &EditorOperation::Path(path));
            ctx.operations
                .serialize_cursors(ctx.target_client, &view.cursors);
        } else if path.to_str().map(|s| s.trim().len()).unwrap_or(0) > 0 {
            let content = match File::open(&path) {
                Ok(mut file) => {
                    let mut content = String::new();
                    match file.read_to_string(&mut content) {
                        Ok(_) => (),
                        Err(error) => {
                            return Err(format!(
                                "could not read contents from file {:?}: {:?}",
                                path, error
                            ))
                        }
                    }
                    BufferContent::from_str(&content[..])
                }
                Err(_) => BufferContent::from_str(""),
            };

            new_buffer_from_content(ctx, path, content);
        } else {
            return Err(format!("invalid path {:?}", path));
        }

        Ok(())
    }

    pub fn write_buffer_to_file(buffer: &Buffer, path: &Path) -> Result<(), String> {
        let mut file =
            File::create(path).map_err(|e| format!("could not create file {:?}: {:?}", path, e))?;

        buffer
            .content
            .write(&mut file)
            .map_err(|e| format!("could not write to file {:?}: {:?}", path, e))
    }
}
