use crate::mode::{poll_input, FromMode, InputResult, ModeContext, Operation};

pub fn on_enter(ctx: ModeContext) {
    ctx.input.clear();
}

pub fn on_event(mut ctx: ModeContext, from_mode: &FromMode) -> Operation {
    match poll_input(&mut ctx) {
        InputResult::Canceled => Operation::EnterMode(from_mode.as_mode()),
        InputResult::Submited => {
            // handle command here
            Operation::EnterMode(from_mode.as_mode())
        }
        InputResult::Pending => Operation::None,
    }
}
