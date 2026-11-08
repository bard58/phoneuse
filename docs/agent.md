# The agent: `agentphone run`

`agentphone run` gives a task to an AI model. The model looks at the screen,
selects an action, and agentphone does the action on the phone. This repeats
until the task is done, the step limit is reached, or you stop the run.

```sh
agentphone run "open Settings and turn on Wi-Fi"
```

To use the phone from Claude Code or a different MCP client instead, see
[mcp.md](mcp.md). MCP mode needs no API key in agentphone.

## Options

| Option | What it does |
|---|---|
| `--max-steps N` | Stops after N AI steps. The default is `safety.max_steps` (50). |
| `--model ID` | Uses this model for this run, for example `claude-sonnet-5-5`. |
| `--provider NAME` | Uses `anthropic` or `openai` (OpenAI-compatible) for this run. |
| `--device ID` | Uses this phone. |
| `--json` | Prints the result as JSON. |
| `--jev` | Also uses Jev for quick screen checks. Only in builds with the `jev` feature. |

At the end, agentphone prints the outcome, the number of AI steps, the input
and output tokens, and an estimated cost.

## Providers and models

### Claude (default)

The default provider is `anthropic` and the default model is `claude-opus-5-5`.

1. Get an API key at <https://console.anthropic.com/settings/keys>.
2. Set the environment variable `ANTHROPIC_API_KEY`:

   ```powershell
   setx ANTHROPIC_API_KEY "paste-your-key-here"
   ```

   On macOS and Linux, add `export ANTHROPIC_API_KEY="..."` to the shell profile.
3. Open a new terminal.

To change the model:

```sh
agentphone config set llm.model claude-sonnet-5-5
```

### OpenAI-compatible APIs

agentphone can use any API that has the OpenAI `/chat/completions` format with
tools and images. The model must accept images.

1. Set the provider to `openai`:

   ```sh
   agentphone config set llm.provider openai
   ```

2. Set the address and the model:

   | Service | `llm.base_url` | Example `llm.model` |
   |---|---|---|
   | OpenAI | `none` (default `https://api.openai.com/v1`) | a vision model |
   | OpenRouter | `https://openrouter.ai/api/v1` | a vision model id |
   | Ollama | `http://localhost:11434/v1` | a local vision model |
   | LM Studio | `http://localhost:1234/v1` | a local vision model |

   ```sh
   agentphone config set llm.base_url http://localhost:11434/v1
   agentphone config set llm.model <model-name>
   ```

3. Set `OPENAI_API_KEY` to the key of the service. A local server (localhost)
   needs no key.

Use `llm.provider openai` for all of these services, or the names
`openrouter`, `ollama`, and `lmstudio`. With these three names, an empty
`llm.base_url` gives the address in the table above. Thus `ollama` and
`lmstudio` use localhost by default, not `api.openai.com`. Every provider
other than `anthropic` uses the OpenAI format and reads the key from
`OPENAI_API_KEY` (change the name with `llm.openai_api_key_env`). With
`ollama`, `lmstudio`, or a localhost address, a missing key is not an error.

`agentphone setup llm` asks these questions for you.

## Environment variables

API keys come from the environment only. agentphone never saves a key in the
config file. `agentphone config set llm.api_key ...` is refused.

| Variable | Use |
|---|---|
| `ANTHROPIC_API_KEY` | Claude API key. Change the name with `llm.anthropic_api_key_env`. |
| `OPENAI_API_KEY` | OpenAI-compatible API key. Change the name with `llm.openai_api_key_env`. |
| `AGENTPHONE_DEVICE` | The phone to use (same as `--device`). |
| `AGENTPHONE_CONFIG` | A different config file (same as `--config`). |
| `AGENTPHONE_DDI_MIRROR` | A different download address for the iPhone DDI. |
| `TYPESAFE_API_KEY`, `TYPESAFE_MODEL`, `TYPESAFE_BASE_URL` | Jev settings (optional feature). |

## Cost

agentphone prints an estimate after each run. The estimate uses these prices
(USD for each million tokens). Prices can change. Examine the prices of your
provider.

| Model | Input | Output |
|---|---|---|
| `claude-opus-5-5` | 4.00 | 20.00 |
| `claude-sonnet-5-5` | 2.00 | 10.00 |
| `claude-haiku-5-5` | 0.10 | 0.50 |

For OpenAI-compatible and local models, agentphone shows tokens but no cost.
Each step sends one downscaled screenshot. A long task costs more. To set a
limit, use `--max-steps` or `safety.max_steps`.

Limits of the estimate:

- When Claude answers from a fallback model, agentphone counts the tokens of
  all attempts. It uses the price of the model that gave the answer for all of
  them.
- If you stop the run while a model call runs, the provider can still bill
  that call. agentphone does not know its tokens. The summary then says
  "(+N cancelled call not counted; the cost is a lower bound)".

### Time limits and retries

- Each model request has a time limit of at least 600 seconds. For a large
  `max_tokens`, the limit is longer.
- agentphone does not send a request again after a time-out, because the
  provider can bill the first request.
- agentphone sends a request again after a connection failure, a rate limit
  (429), or an overloaded server. It obeys the `retry-after` delay of the
  server up to 60 seconds. For a longer delay, it stops.
- agentphone does not retry an error for an empty quota or billing
  (`insufficient_quota`, `billing_hard_limit_reached`, `billing_not_active`).

## Safety rules

These rules lower the risk. They do not remove it. Some rules depend on the
AI model, and some depend on what is on the screen. Ctrl+C is the hard stop.

### Confirmation

This is on by default (`safety.confirm_irreversible = true`). You answer in
the terminal. agentphone asks you before an action in these cases:

- The model marks the action as irreversible. The model must do this before
  it sends, posts, pays, buys, deletes, or changes an account, privacy, or
  security setting.
- The text to type contains a newline.
- The key is Enter, with or without modifier keys.
- The action is a tap, double tap, or long press right after typing, and the
  open app is a messaging, payment, or shopping app. If agentphone cannot tell
  which app is open, it also asks.

agentphone itself makes the last three checks. They do not depend on the
model.

The question shows the next step, for example "Next step: tap at (0.900,
0.900)". A yes allows only that one call. If the next call is a different
action, agentphone refuses it and cancels the approval. A look, a wait, a
question, or an unlock also cancels the approval.

After you answer no, agentphone allows only Home and Back. This stays so until
you answer a question from the agent, or approve a new confirmation.

> CAUTION: Do not set `safety.confirm_irreversible` to `false`. The agent can
> then send messages or buy items without a question. The checks for Enter and
> newlines also stop.

### Passcodes

- The agent never asks for, reads, or types a passcode, PIN, password, or
  one-time code. If the phone is locked, the agent stops and asks you to
  unlock it.
- agentphone counts the digits that the agent types, with `type_text` and
  with digit keys. At 4 digits or more, it refuses the action. Spaces, dashes,
  and dots between the digits do not hide them. Full-width and other Unicode
  digits also count. A tap between the digits does not reset the count.
- To allow digits (for example a phone number), approve the exact call in the
  terminal. agentphone asks this also when confirmations are off.
- When the phone looks locked, agentphone refuses all text and keys. "Looks
  locked" means that the screen text has a lock or passcode phrase, or that
  an unlock did not end with "unlocked" or "not locked". The screen text needs
  OCR. Without OCR, only the unlock result counts.
- agentphone does not find a password that has letters, for example
  `hunter2!`.
- `safety.allow_unlock` is off by default. When it is on, the
  `agentphone run` terminal still does not enter a passcode: the agent asks
  you to unlock the phone by hand.
- The step lines in the terminal hide digit text and digit keys. The model
  sees the real values.

### Take-over

- Push Ctrl+C at any time. The first Ctrl+C stops the agent, and you have
  control of the phone. A second Ctrl+C exits at once.
- The first Ctrl+C stops the current action and the model call. `type_text`
  stops before the next part of the text, and never pushes Enter after a
  stop.
- On an iPhone, the phone can complete a part of an action that it already
  received. Each iPhone call has a time limit, so a stalled call cannot hold
  the phone for good.

### Other rules

- If the screenshot after an action fails, the result is an error. agentphone
  refuses the next action until a look succeeds.
- On an iPhone, agentphone does not send input again after the phone got a
  part of it. The result is then an "input may be partly delivered" error.
- The run stops at `safety.max_steps` (default 50).

## Jev (optional)

Jev is a small TypeSafe model that gives quick screen checks: is the screen
still, is the phone locked, is a keyboard or an alert visible.

> WARNING: Jev sends screenshots of your phone to typesafe.ai, a third-party
> service. Use it only if you accept this.

1. Build with the feature: `cargo install --git https://github.com/bard58/phoneuse --branch rust-rewrite ap-cli --features jev`.
2. Get an API key at <https://typesafe.ai> and set `TYPESAFE_API_KEY`.
3. Run `agentphone run --jev "TASK"`.
4. Answer yes when agentphone asks for permission. It asks at each run.

Jev is off by default. Before each Jev call, agentphone finds the app on the
screen and does not send the screenshot in these cases:

- agentphone cannot tell which app is open. On Android, agentphone reads the
  app from `dumpsys window`. When the notification shade or the lock screen
  has the focus, the app is unknown. On an iPhone, agentphone knows the app
  only right after it opens it with `open_app`. After any input (a tap, a
  swipe, text, a key, or a button), the app is unknown. Thus on an iPhone,
  Jev runs only on the first look after `open_app`.
- The app id contains a word from the blocklist, for example `bank`, `paypal`,
  `wallet`, `messages`, `whatsapp`, `gmail`, `password`, `authenticator`,
  `photos`, or `health` (see `DEFAULT_BLOCKLIST` in
  `crates/ap-jev/src/privacy.rs`).

The blocklist compares words with the app id. A banking app with an id that
contains no blocklist word is sent. The list also has real Android package
ids, for example `com.google.android.gm` (Gmail), `org.thoughtcrime.securesms`
(Signal), and `com.chase.sig.android` (Chase).

On Android, the guard sees only the package id, not the app name. A word such
as `gmail` can match an iOS bundle id, but not the Android id of Gmail. If you
add an entry for an Android app, use its package id.

The consent question needs a terminal. With no terminal (for example piped
input), `--jev` stops with an error and does not ask.


