use std::fs;

use crate::{
    client::ClientHandle,
    command::{CommandManager, CommandTokenizer, CompletionSource},
    editor::{Editor, EditorContext, EditorFlow, KeysIterator},
    editor_utils::{hash_bytes, ReadLinePoll},
    mode::{ModeKind, ModeState},
    picker::Picker,
    platform::{Key, KeyCode},
    word_database::WordIndicesIter,
};

enum ReadCommandState {
    NavigatingHistory(usize),
    TypingCommand,
}

pub struct State {
    read_state: ReadCommandState,
    completion_index: usize,
    completion_source: CompletionSource,
    completion_path_hash: Option<u64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            read_state: ReadCommandState::TypingCommand,
            completion_index: 0,
            completion_source: CompletionSource::Custom(&[]),
            completion_path_hash: None,
        }
    }
}

impl ModeState for State {
    fn on_enter(editor: &mut Editor) {
        let state = &mut editor.mode.command_state;
        state.read_state = ReadCommandState::NavigatingHistory(editor.commands.history_len());
        state.completion_index = 0;
        state.completion_source = CompletionSource::Custom(&[]);
        state.completion_path_hash = None;

        editor.read_line.set_prompt(":");
        editor.read_line.input_mut().clear();
        editor.picker.clear();
    }

    fn on_exit(editor: &mut Editor) {
        editor.read_line.input_mut().clear();
        editor.picker.clear();
    }

    fn on_keys(
        ctx: &mut EditorContext,
        client_handle: ClientHandle,
        keys: &mut KeysIterator,
    ) -> Option<EditorFlow> {
        let state = &mut ctx.editor.mode.command_state;
        match ctx.editor.read_line.poll(
            &mut ctx.platform,
            &mut ctx.editor.string_pool,
            &ctx.editor.buffered_keys,
            keys,
        ) {
            ReadLinePoll::Pending => {
                keys.index = keys.index.saturating_sub(1);
                match keys.next(&ctx.editor.buffered_keys) {
                    Key {
                        code: KeyCode::Char('n' | 'j'),
                        shift: false,
                        control: true,
                        alt: false,
                    }
                    | Key {
                        code: KeyCode::Down,
                        shift: false,
                        control: false,
                        alt: false,
                    } => match state.read_state {
                        ReadCommandState::NavigatingHistory(ref mut i) => {
                            *i = ctx
                                .editor
                                .commands
                                .history_len()
                                .saturating_sub(1)
                                .min(*i + 1);
                            let entry = ctx.editor.commands.history_entry(*i);
                            let input = ctx.editor.read_line.input_mut();
                            input.clear();
                            input.push_str(entry);
                        }
                        ReadCommandState::TypingCommand => apply_completion(ctx, 1),
                    },
                    Key {
                        code: KeyCode::Char('p' | 'k'),
                        shift: false,
                        control: true,
                        alt: false,
                    }
                    | Key {
                        code: KeyCode::Up,
                        shift: false,
                        control: false,
                        alt: false,
                    } => match state.read_state {
                        ReadCommandState::NavigatingHistory(ref mut i) => {
                            *i = i.saturating_sub(1);
                            let entry = ctx.editor.commands.history_entry(*i);
                            let input = ctx.editor.read_line.input_mut();
                            input.clear();
                            input.push_str(entry);
                        }
                        ReadCommandState::TypingCommand => apply_completion(ctx, -1),
                    },
                    _ => update_autocomplete_entries(ctx),
                }
            }
            ReadLinePoll::Canceled => ctx.editor.enter_mode(ModeKind::default()),
            ReadLinePoll::Submitted => {
                let input = ctx.editor.read_line.input();
                ctx.editor.commands.add_to_history(input);

                let command = ctx.editor.string_pool.acquire_with(input);
                ctx.editor.enter_mode(ModeKind::default());

                let result =
                    CommandManager::eval(ctx, Some(client_handle), "readline-command", &command);
                let flow = CommandManager::unwrap_eval_result(ctx, result);
                ctx.editor.string_pool.release(command);

                return Some(flow);
            }
        }

        Some(EditorFlow::Continue)
    }
}

fn apply_completion(ctx: &mut EditorContext, cursor_movement: isize) {
    ctx.editor.picker.move_cursor(cursor_movement);
    if let Some((_, entry)) = ctx.editor.picker.current_entry(&ctx.editor.word_database) {
        let input = ctx.editor.read_line.input_mut();
        input.truncate(ctx.editor.mode.command_state.completion_index);
        input.push_str(entry);
    }
}

fn update_autocomplete_entries(ctx: &mut EditorContext) {
    let state = &mut ctx.editor.mode.command_state;

    let input = ctx.editor.read_line.input();
    let mut tokens = CommandTokenizer(input);

    let mut last_token = match tokens.next() {
        Some(token) => token,
        None => {
            ctx.editor.picker.clear();
            state.completion_index = input.len();
            state.completion_source = CompletionSource::Custom(&[]);
            if input.trim().is_empty() {
                state.read_state =
                    ReadCommandState::NavigatingHistory(ctx.editor.commands.history_len());
            }
            return;
        }
    };
    let command_name = last_token.slice.trim_end_matches('!');

    if let ReadCommandState::NavigatingHistory(_) = state.read_state {
        state.read_state = ReadCommandState::TypingCommand;
    }
    ctx.editor.picker.clear_cursor();

    let mut arg_count = 0;

    for token in tokens {
        arg_count += 1;
        last_token = token;
    }

    let mut pattern = last_token.slice;

    let ends_with_whitespace = input.ends_with(&[' ', '\t']);
    if ends_with_whitespace {
        arg_count += 1;
        pattern = &input[input.len()..];
    }

    let mut completion_source = CompletionSource::Custom(&[]);
    if arg_count > 0 {
        if let Some(command) = ctx.editor.commands.find_command(command_name) {
            let completion_index = arg_count - 1;
            if completion_index < command.completions.len() {
                completion_source = command.completions[completion_index];
            }
        }
    } else {
        completion_source = CompletionSource::Commands;
    }

    if last_token.is_simple && !last_token.can_expand_variables && last_token.slice.is_empty() {
        completion_source = CompletionSource::Expansions;
    } else if last_token.can_expand_variables {
        if let Some(i) = pattern.rfind('@') {
            let rest = &pattern[i + 1..];
            if rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            {
                pattern = rest;
                completion_source = CompletionSource::Expansions;
            }
        }
    }

    state.completion_index = pattern.as_ptr() as usize - input.as_ptr() as usize;

    if state.completion_source != completion_source {
        state.completion_path_hash = None;
        ctx.editor.picker.clear();

        match completion_source {
            CompletionSource::Commands => {
                for command_name in ctx.editor.commands.command_names() {
                    ctx.editor.picker.add_custom_entry(command_name);
                }
                for name in ctx.editor.commands.macros.names() {
                    if !name.starts_with('-') {
                        ctx.editor.picker.add_custom_entry(name);
                    }
                }
            }
            CompletionSource::Expansions => {
                for expansion_name in ctx.editor.commands.expansion_names() {
                    ctx.editor.picker.add_custom_entry(expansion_name);
                }
            }
            CompletionSource::Buffers => {
                for buffer in ctx.editor.buffers.iter() {
                    if let Some(path) = buffer.path.to_str() {
                        ctx.editor.picker.add_custom_entry(path);
                    }
                }
            }
            CompletionSource::Custom(completions) => {
                for completion in completions {
                    ctx.editor.picker.add_custom_entry(completion);
                }
            }
            _ => (),
        }
    }

    if let CompletionSource::Files = completion_source {
        fn set_files_in_path_as_entries(picker: &mut Picker, path: &str) {
            picker.clear();
            let path = if path.is_empty() { "." } else { path };
            let read_dir = match fs::read_dir(path) {
                Ok(iter) => iter,
                Err(_) => return,
            };
            for entry in read_dir {
                let entry = match entry {
                    Ok(entry) => entry.file_name(),
                    Err(_) => return,
                };
                if let Some(entry) = entry.to_str() {
                    picker.add_custom_entry(entry);
                }
            }
        }

        let (parent, file) = match pattern.rfind('/') {
            Some(i) => pattern.split_at(i + 1),
            None => ("", pattern),
        };

        let parent_hash = hash_bytes(parent.as_bytes());
        if state.completion_path_hash != Some(parent_hash) {
            set_files_in_path_as_entries(&mut ctx.editor.picker, parent);
            state.completion_path_hash = Some(parent_hash);
        }

        state.completion_index = file.as_ptr() as usize - input.as_ptr() as usize;
        pattern = file;
    }

    state.completion_source = completion_source;
    ctx.editor.picker.filter(WordIndicesIter::empty(), pattern);
}
