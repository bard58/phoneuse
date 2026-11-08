//! The server `instructions` string (a short version of upstream SKILL.md).

use crate::ServerOptions;

const BASE: &str = "\
Operate the user's real phone (iPhone or Android) through these tools.

The loop: look, decide, act, look again. Every action (tap, swipe, type_text, ...) returns the screen in the same call: \
up to three frames taken 0.1, 0.4 and 0.8 s after it, side by side in one image (frames that did not change are dropped, \
so a still screen is one frame), plus, when OCR is available, the last frame's text lines as \"x y  text\". \
You see motion yourself (a page sliding in, a spinner, video). If the last frame is not final yet, call wait(seconds) \
with a duration you choose. Call look only when you need the screen without acting.

Coordinates: every x and y is a normalized fraction of the screen, 0.0 to 1.0 from the top-left corner \
(centre = 0.5, 0.5). In a multi-frame strip, measure inside one frame, not across the whole image. \
look(grid=true) draws a labelled 0.1 grid to help you read positions.

Prefer tap_on for visible text when OCR is available; it taps the exact position of the text and refuses text \
that is not on screen (scroll first). Typing replaces what the focused field holds unless replace=false. \
The phone may autocorrect: check the typed text before you send anything.

Apps reopen where they were left (a sheet, a sub-page): check the first screen after open_app, and close leftovers \
or navigate back to the app's main view.

Call status first if anything fails; errors that start with a problem and a \"fix:\" line need the user to do \
something on the PC or the phone: tell them the fix. When your task is done, call end_session to release the phone.";

const RULES: &str = "\
Rules:
- This is the user's personal phone. Messages, notifications and photos are private: use only what the task needs \
and do not repeat other contents.
- Ask the user in chat and wait for a clear yes before anything that sends, posts, buys, pays, deletes, \
or changes an account, privacy or security setting.
- Permission prompts (\"Allow notifications?\") belong to the user: leave them or ask, unless the task is about that permission.
- Never type, guess or ask for a passcode, PIN or password. On the lock screen, stop and ask the user to unlock the phone.
- If the screen changes on its own, the user may be using the phone: stop and ask before you continue.";

/// The instructions for these options.
pub fn instructions(opts: &ServerOptions) -> String {
    let mut s = format!("{BASE}\n\n{RULES}");
    if let Some(extra) = opts.extra_instructions.as_deref().map(str::trim)
        && !extra.is_empty()
    {
        s.push_str("\n\n");
        s.push_str(extra);
    }
    s
}
