use std::{
    fmt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    application::ProcessTag,
    buffer::{BufferCapabilities, BufferHandle},
    buffer_position::BufferPosition,
    buffer_view::BufferViewError,
    command::{
        BuiltinCommand, CommandContext, CommandError, CommandOperation, CommandSource,
        CompletionSource,
    },
    config::{ParseConfigError, CONFIG_NAMES},
    editor::Editor,
    editor_utils::MessageKind,
    json::Json,
    keymap::ParseKeyMapError,
    lsp,
    mode::ModeKind,
    navigation_history::NavigationHistory,
    platform::{Platform, PlatformRequest},
    register::RegisterKey,
    syntax::{Syntax, TokenKind},
    theme::{Color, THEME_COLOR_NAMES},
};

pub const COMMANDS: &[BuiltinCommand] = &[
    BuiltinCommand {
        names: &["help", "h"],
        description: "prints help about command",
        bang_usage: None,
        params: &[("command-name", Some(CompletionSource::Commands))],
        func: |ctx| {
            let command_name = ctx.args[0];
            let commands = &ctx.editor.commands;
            let source = match commands.find_command(command_name) {
                Some(source) => source,
                None => return Err(CommandError::CommandNotFound(command_name)),
            };

            let name;
            let aliases;
            let description;
            let bang_usage;
            let params;

            match source {
                CommandSource::Builtin(i) => {
                    let command = &commands.builtin_commands()[i];
                    name = command.names[0];
                    aliases = &command.names[1..];
                    description = command.description;
                    bang_usage = command.bang_usage;
                    params = command.params;
                }
            }

            let mut write = ctx.editor.status_bar.write(MessageKind::Info);

            write.fmt(format_args!("{}\nusage: {}", name, name));
            if bang_usage.is_some() {
                write.str("[!]");
            }
            for (param, _) in params {
                write.fmt(format_args!(" {}", param));
            }

            write.fmt(format_args!("\ndescription: {}\n", description));
            if let Some(usage) = bang_usage {
                write.fmt(format_args!("with '!': {}\n", usage));
            }

            if !aliases.is_empty() {
                write.str("aliases: ");
                write.fmt(format_args!("{}", aliases[0]));
                for alias in &aliases[1..] {
                    write.fmt(format_args!(", {}", alias));
                }
            }

            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["quit", "q"],
        description: "quits this client",
        bang_usage: Some("ignore unsaved changes"),
        params: &[],
        func: |ctx| {
            if ctx.clients.iter_mut().count() == 1 {
                ctx.assert_can_discard_all_buffers()?;
            }
            Ok(Some(CommandOperation::Quit))
        },
    },
    BuiltinCommand {
        names: &["quit-all", "qa"],
        description: "quits all clients",
        bang_usage: Some("ignore unsaved changes"),
        params: &[],
        func: |ctx| {
            ctx.assert_can_discard_all_buffers()?;
            Ok(Some(CommandOperation::QuitAll))
        },
    },
    BuiltinCommand {
        names: &["print", "p"],
        description: "prints values to the status bar",
        bang_usage: None,
        params: &[("message", None)],
        func: |ctx| {
            let message = ctx.args[0];
            ctx.editor.status_bar.write(MessageKind::Info).str(message);
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["source"],
        description: "load a source file and execute its commands",
        bang_usage: None,
        params: &[("path", Some(CompletionSource::Files))],
        func: |ctx| {
            let path = ctx.args[0];
            let op = ctx.editor.load_config(ctx.platform, ctx.clients, path);
            Ok(op)
        },
    },
    BuiltinCommand {
        names: &["open", "o"],
        description: "open a buffer for editting",
        bang_usage: None,
        params: &[("path", Some(CompletionSource::Files))],
        func: |ctx| {
            let client_handle = ctx.client_handle.ok_or(CommandError::Aborted)?;
            NavigationHistory::save_client_snapshot(
                ctx.clients,
                client_handle,
                &ctx.editor.buffer_views,
            );

            let mut path = ctx.args[0];
            let mut line_index = None;
            if let Some(separator_index) = path.rfind(':') {
                if let Ok(n) = path[(separator_index + 1)..].parse() {
                    let n: usize = n;
                    line_index = Some(n.saturating_sub(1));
                    path = &path[..separator_index];
                }
            }

            match ctx.editor.buffer_views.buffer_view_handle_from_path(
                client_handle,
                &mut ctx.editor.buffers,
                &mut ctx.editor.word_database,
                &ctx.editor.current_directory,
                Path::new(path),
                line_index,
                &mut ctx.editor.events,
            ) {
                Ok(handle) => {
                    ctx.clients
                        .get_mut(client_handle)
                        .ok_or(CommandError::Aborted)?
                        .set_buffer_view_handle(Some(handle));
                    use fmt::Write;
                    let _ = write!(ctx.output, "{}", handle);
                    Ok(None)
                }
                Err(BufferViewError::InvalidPath) => Err(CommandError::InvalidPath(path)),
            }
        },
    },
    BuiltinCommand {
        names: &["save", "s"],
        description: "save current buffer",
        bang_usage: None,
        params: &[],
        func: |ctx| {
            save_buffer(ctx, None, None)?;
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["save-to", "st"],
        description: "save current buffer to new path",
        bang_usage: None,
        params: &[("path", Some(CompletionSource::Files))],
        func: |ctx| {
            let path = Path::new(ctx.args[0]);
            save_buffer(ctx, None, Some(path))?;
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["save-buffer-to"],
        description: "save buffer to path",
        bang_usage: None,
        params: &[("buffer", None), ("path", Some(CompletionSource::Files))],
        func: |ctx| {
            let buffer_handle = ctx.parse_arg(0)?;
            let path = Path::new(ctx.args[1]);
            save_buffer(ctx, Some(buffer_handle), Some(path))?;
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["save-all", "sa"],
        description: "save all buffers",
        bang_usage: None,
        params: &[],
        func: |ctx| {
            let mut count = 0;
            for buffer in ctx.editor.buffers.iter_mut() {
                buffer
                    .save_to_file(None, &mut ctx.editor.events)
                    .map_err(|e| CommandError::BufferError(buffer.handle(), e))?;
                count += 1;
            }
            ctx.editor
                .status_bar
                .write(MessageKind::Info)
                .fmt(format_args!("{} buffers saved", count));
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["reload", "r"],
        description: "reload current buffer from file",
        bang_usage: Some("ignore unsaved changes"),
        params: &[],
        func: |ctx| {
            reload_buffer(ctx, None)?;
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["reload-buffer"],
        description: "reload buffer from file",
        bang_usage: Some("ignore unsaved changes"),
        params: &[("buffer", None)],
        func: |ctx| {
            let buffer_handle = ctx.parse_arg(0)?;
            reload_buffer(ctx, Some(buffer_handle))?;
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["reload-all", "ra"],
        description: "reload all buffers from file",
        bang_usage: Some("ignore unsaved changes"),
        params: &[],
        func: |ctx| {
            ctx.assert_can_discard_all_buffers()?;
            let mut count = 0;
            for buffer in ctx.editor.buffers.iter_mut() {
                buffer
                    .discard_and_reload_from_file(
                        &mut ctx.editor.word_database,
                        &mut ctx.editor.events,
                    )
                    .map_err(|e| CommandError::BufferError(buffer.handle(), e))?;
                count += 1;
            }
            ctx.editor
                .status_bar
                .write(MessageKind::Info)
                .fmt(format_args!("{} buffers reloaded", count));
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["close", "c"],
        description: "close current buffer",
        bang_usage: Some("ignore unsaved changes"),
        params: &[],
        func: |ctx| {
            close_buffer(ctx, None)?;
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["close-buffer"],
        description: "close buffer",
        bang_usage: Some("ignore unsaved changes"),
        params: &[],
        func: |ctx| {
            let buffer_handle = ctx.parse_arg(0)?;
            close_buffer(ctx, Some(buffer_handle))?;
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["close-all", "ca"],
        description: "close all buffers",
        bang_usage: Some("ignore unsaved changes"),
        params: &[],
        func: |ctx| {
            ctx.assert_can_discard_all_buffers()?;
            let count = ctx.editor.buffers.iter().count();
            ctx.editor.buffer_views.defer_remove_buffer_where(
                &mut ctx.editor.buffers,
                &mut ctx.editor.events,
                |_| true,
            );

            for client in ctx.clients.iter_mut() {
                client.set_buffer_view_handle(None);
            }

            ctx.editor
                .status_bar
                .write(MessageKind::Info)
                .fmt(format_args!("{} buffers closed", count));
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["get-config"],
        description: "get an editor config",
        bang_usage: None,
        params: &[("key", Some(CompletionSource::Custom(CONFIG_NAMES)))],
        func: |ctx| {
            let key = ctx.args[0];
            match ctx.editor.config.display_config(key) {
                Some(display) => {
                    use fmt::Write;
                    let _ = write!(ctx.output, "{}", display);
                    Ok(None)
                }
                None => Err(CommandError::ConfigNotFound(key)),
            }
        },
    },
    BuiltinCommand {
        names: &["set-config"],
        description: "set an editor config",
        bang_usage: None,
        params: &[
            ("key", Some(CompletionSource::Custom(CONFIG_NAMES))),
            ("value", None),
        ],
        func: |ctx| {
            let key = ctx.args[0];
            let value = ctx.args[1];
            match ctx.editor.config.parse_config(key, value) {
                Ok(()) => Ok(None),
                Err(ParseConfigError::NotFound) => Err(CommandError::ConfigNotFound(key)),
                Err(ParseConfigError::InvalidValue) => {
                    Err(CommandError::InvalidConfigValue { key, value })
                }
            }
        },
    },
    BuiltinCommand {
        names: &["get-color"],
        description: "get editor theme color",
        bang_usage: None,
        params: &[("key", Some(CompletionSource::Custom(THEME_COLOR_NAMES)))],
        func: |ctx| {
            let key = ctx.args[0];
            let color = ctx
                .editor
                .theme
                .color_from_name(key)
                .ok_or(CommandError::ConfigNotFound(key))?;

            use fmt::Write;
            let _ = write!(ctx.output, "{:x}", color.into_u32());
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["set-color"],
        description: "set editor theme color",
        bang_usage: None,
        params: &[
            ("key", Some(CompletionSource::Custom(THEME_COLOR_NAMES))),
            ("value", None),
        ],
        func: |ctx| {
            let key = ctx.args[0];
            let value = ctx.args[1];
            let color = ctx
                .editor
                .theme
                .color_from_name(key)
                .ok_or(CommandError::ConfigNotFound(key))?;
            let encoded = u32::from_str_radix(value, 16)
                .map_err(|_| CommandError::InvalidColorValue { key, value })?;
            *color = Color::from_u32(encoded);
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["syntax"],
        description: "create a syntax definition with patterns for files that match a glob",
        bang_usage: None,
        params: &[
            ("glob", None),
            ("keywords", None),
            ("types", None),
            ("symbols", None),
            ("literals", None),
            ("strings", None),
            ("comments", None),
            ("texts", None),
        ],
        func: |ctx| {
            let glob = ctx.args[0];

            let mut syntax = Syntax::new();
            syntax
                .set_glob(glob.as_bytes())
                .map_err(|_| CommandError::InvalidGlob(glob))?;

            let kinds = [
                TokenKind::Keyword,
                TokenKind::Type,
                TokenKind::Symbol,
                TokenKind::Literal,
                TokenKind::String,
                TokenKind::Comment,
                TokenKind::Text,
            ];
            for (&arg, &kind) in ctx.args[1..].iter().zip(kinds.iter()) {
                if !arg.is_empty() {
                    syntax
                        .set_rule(kind, arg)
                        .map_err(|e| CommandError::PatternError(arg, e))?;
                }
            }

            ctx.editor.syntaxes.add(syntax);
            for buffer in ctx.editor.buffers.iter_mut() {
                buffer.refresh_syntax(&ctx.editor.syntaxes);
            }

            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["keymap"],
        description: "create a keyboard mapping for an editor mode",
        bang_usage: None,
        params: &[
            (
                "mode",
                Some(CompletionSource::Custom(&[
                    "normal",
                    "insert",
                    "read-line",
                    "picker",
                    "command",
                ])),
            ),
            ("from", None),
            ("to", None),
        ],
        func: |ctx| {
            let mode = ctx.args[0];
            let from = ctx.args[1];
            let to = ctx.args[2];

            let mode = match mode {
                "normal" => ModeKind::Normal,
                "insert" => ModeKind::Insert,
                "read-line" => ModeKind::ReadLine,
                "picker" => ModeKind::Picker,
                "command" => ModeKind::Command,
                _ => return Err(CommandError::InvalidModeError(mode)),
            };

            ctx.editor
                .keymaps
                .parse_and_map(mode, from, to)
                .map_err(|e| match e {
                    ParseKeyMapError::From(e) => {
                        CommandError::KeyParseError(&from[e.index..], e.error)
                    }
                    ParseKeyMapError::To(e) => CommandError::KeyParseError(&to[e.index..], e.error),
                })?;

            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["get-register"],
        description: "get an editor register",
        bang_usage: None,
        params: &[("key", None)],
        func: |ctx| {
            let key = ctx.args[0];
            match RegisterKey::from_str(key) {
                Some(key) => {
                    let register = ctx.editor.registers.get_mut(key);
                    ctx.output.push_str(register);
                    Ok(None)
                }
                None => Err(CommandError::InvalidRegisterKey(key)),
            }
        },
    },
    BuiltinCommand {
        names: &["set-register"],
        description: "set an editor register",
        bang_usage: None,
        params: &[("key", None), ("value", None)],
        func: |ctx| {
            let key = ctx.args[0];
            let value = ctx.args[1];
            match RegisterKey::from_str(key) {
                Some(key) => {
                    let register = ctx.editor.registers.get_mut(key);
                    register.clear();
                    register.push_str(value);
                    Ok(None)
                }
                None => Err(CommandError::InvalidRegisterKey(key)),
            }
        },
    },
    // TODO: remove this command
    BuiltinCommand {
        names: &["run"],
        description: "",
        bang_usage: None,
        params: &[("command", None)],
        func: |ctx| {
            let command = ctx.args[0];
            eprintln!("request spawn process '{}'", command);

            let mut command = Command::new(command);
            command.stdin(Stdio::null());
            command.stdout(Stdio::piped());
            command.stderr(Stdio::null());

            ctx.platform.enqueue_request(PlatformRequest::SpawnProcess {
                tag: ProcessTag::Command(0),
                command,
                stdout_buf_len: 4 * 1024,
                stderr_buf_len: 0,
            });
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["lsp-start"],
        description: "start a lsp server",
        bang_usage: None,
        params: &[
            ("server-command", None),
            ("root", Some(CompletionSource::Files)),
        ],
        func: |ctx| {
            let server_command = ctx.args[0];
            let root = PathBuf::from(ctx.args[1]);

            // TODO: handle server command args
            let command = Command::new(server_command);

            let handle = ctx.editor.lsp.start(ctx.platform, command, root);
            use fmt::Write;
            let _ = write!(ctx.output, "{}", handle);
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["lsp-start-with-log"],
        description: "starts a lsp server",
        bang_usage: None,
        params: &[
            ("server-command", None),
            ("root", Some(CompletionSource::Files)),
        ],
        func: |ctx| {
            let server_command = ctx.args[0];
            let root = PathBuf::from(ctx.args[1]);

            // TODO: handle server command args
            let command = Command::new(server_command);

            let handle = ctx.editor.lsp.start(ctx.platform, command, root);
            let clients = &mut *ctx.clients;
            let client_handle = ctx.client_handle;

            lsp::ClientManager::access(ctx.editor, handle, |editor, client, _| {
                let buffer = editor.buffers.new(BufferCapabilities::log());
                let buffer_handle = buffer.handle();
                // TODO: better lsp log buffer path
                buffer.set_path(Some(Path::new("lsp-server-log")));
                client.set_log_buffer(Some(buffer_handle));

                if let Some(client_handle) = client_handle {
                    let buffer_view_handle = editor
                        .buffer_views
                        .buffer_view_handle_from_buffer_handle(client_handle, buffer_handle);
                    if let Some(client) = clients.get_mut(client_handle) {
                        client.set_buffer_view_handle(Some(buffer_view_handle));
                    }
                }
            });

            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["lsp-stop"],
        description: "stops the lsp server associated with the current buffer",
        bang_usage: None,
        params: &[],
        func: |ctx| {
            let buffer_handle = ctx.current_buffer_handle()?;
            match find_lsp_client_for_buffer(ctx.editor, buffer_handle) {
                Some(client) => ctx.editor.lsp.stop(ctx.platform, client),
                None => ctx.editor.lsp.stop_all(ctx.platform),
            }
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["lsp-hover"],
        description: "performs a lsp hover action at the current buffer's main cursor position",
        bang_usage: None,
        params: &[],
        func: |mut ctx| {
            let (buffer_handle, position) = current_buffer_and_main_position(&ctx)?;
            access_lsp(&mut ctx, buffer_handle, |editor, platform, client, json| {
                client.hover(editor, platform, json, buffer_handle, position)
            })?;
            Ok(None)
        },
    },
    BuiltinCommand {
        names: &["lsp-signature-help"],
        description:
            "performs a lsp signature help action at the current buffer's main cursor position",
        bang_usage: None,
        params: &[],
        func: |mut ctx| {
            let (buffer_handle, position) = current_buffer_and_main_position(&ctx)?;
            access_lsp(&mut ctx, buffer_handle, |editor, platform, client, json| {
                client.signature_help(editor, platform, json, buffer_handle, position)
            })?;
            Ok(None)
        },
    },
];

fn save_buffer<'state, 'command>(
    ctx: &mut CommandContext<'state, 'command>,
    buffer_handle: Option<BufferHandle>,
    path: Option<&Path>,
) -> Result<(), CommandError<'command>> {
    let buffer_handle = match buffer_handle {
        Some(handle) => handle,
        None => ctx.current_buffer_handle()?,
    };

    let buffer = ctx
        .editor
        .buffers
        .get_mut(buffer_handle)
        .ok_or(CommandError::InvalidBufferHandle(buffer_handle))?;

    buffer
        .save_to_file(path, &mut ctx.editor.events)
        .map_err(|e| CommandError::BufferError(buffer_handle, e))?;

    let path = buffer.path().unwrap_or(Path::new(""));
    ctx.editor
        .status_bar
        .write(MessageKind::Info)
        .fmt(format_args!("buffer saved to '{:?}'", path));
    Ok(())
}

fn reload_buffer<'state, 'command>(
    ctx: &mut CommandContext<'state, 'command>,
    buffer_handle: Option<BufferHandle>,
) -> Result<(), CommandError<'command>> {
    let buffer_handle = match buffer_handle {
        Some(handle) => handle,
        None => ctx.current_buffer_handle()?,
    };

    ctx.assert_can_discard_buffer(buffer_handle)?;
    let buffer = ctx
        .editor
        .buffers
        .get_mut(buffer_handle)
        .ok_or(CommandError::InvalidBufferHandle(buffer_handle))?;

    buffer
        .discard_and_reload_from_file(&mut ctx.editor.word_database, &mut ctx.editor.events)
        .map_err(|e| CommandError::BufferError(buffer_handle, e))?;

    ctx.editor
        .status_bar
        .write(MessageKind::Info)
        .str("buffer reloaded");
    Ok(())
}

fn close_buffer<'state, 'command>(
    ctx: &mut CommandContext<'state, 'command>,
    buffer_handle: Option<BufferHandle>,
) -> Result<(), CommandError<'command>> {
    let buffer_handle = match buffer_handle {
        Some(handle) => handle,
        None => ctx.current_buffer_handle()?,
    };

    ctx.assert_can_discard_buffer(buffer_handle)?;
    ctx.editor.buffer_views.defer_remove_buffer_where(
        &mut ctx.editor.buffers,
        &mut ctx.editor.events,
        |view| view.buffer_handle == buffer_handle,
    );

    let clients = &mut *ctx.clients;
    let editor = &mut *ctx.editor;
    for client in clients.iter_mut() {
        let maybe_buffer_handle = client
            .buffer_view_handle()
            .and_then(|h| editor.buffer_views.get(h))
            .map(|v| v.buffer_handle);
        if maybe_buffer_handle == Some(buffer_handle) {
            client.set_buffer_view_handle(None);
        }
    }

    editor
        .status_bar
        .write(MessageKind::Info)
        .str("buffer closed");
    Ok(())
}

fn current_buffer_and_main_position<'state, 'command>(
    ctx: &CommandContext<'state, 'command>,
) -> Result<(BufferHandle, BufferPosition), CommandError<'command>> {
    let view_handle = ctx.current_buffer_view_handle()?;
    let buffer_view = ctx
        .editor
        .buffer_views
        .get(view_handle)
        .ok_or(CommandError::NoBufferOpened)?;

    let buffer_handle = buffer_view.buffer_handle;
    let position = buffer_view.cursors.main_cursor().position;
    Ok((buffer_handle, position))
}

fn find_lsp_client_for_buffer(
    editor: &Editor,
    buffer_handle: BufferHandle,
) -> Option<lsp::ClientHandle> {
    let buffer_path_bytes = editor
        .buffers
        .get(buffer_handle)?
        .path()?
        .to_str()?
        .as_bytes();
    let (client_handle, _) = editor
        .lsp
        .client_with_handles()
        .find(|(_, c)| c.handles_path(buffer_path_bytes))?;
    Some(client_handle)
}

fn access_lsp<'command, A>(
    ctx: &mut CommandContext,
    buffer_handle: BufferHandle,
    accessor: A,
) -> Result<(), CommandError<'command>>
where
    A: FnOnce(&mut Editor, &mut Platform, &mut lsp::Client, &mut Json),
{
    let editor = &mut *ctx.editor;
    let platform = &mut *ctx.platform;
    match find_lsp_client_for_buffer(editor, buffer_handle)
        .and_then(|h| lsp::ClientManager::access(editor, h, |e, c, j| accessor(e, platform, c, j)))
    {
        Some(()) => Ok(()),
        None => Err(CommandError::LspServerNotRunning),
    }
}
