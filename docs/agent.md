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
`openrouter`, `ollama`, and `lmstudio`. Every provider other than `anthropic`
reads the key from `OPENAI_API_KEY` (change the name with
`llm.openai_api_key_env`). With `ollama` and `lmstudio`, a missing key is not
an error.

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

## Safety rules

- **Confirmation before irreversible actions.** Before the agent sends, posts,
  pays, buys, deletes, or changes an account, privacy, or security setting, it
  asks you. You answer in the terminal. If you deny, agentphone does not do the
  action. This is on by default (`safety.confirm_irreversible = true`).
- **No passcode.** The agent never asks for, reads, or types a passcode, PIN,
  password, or one-time code. If the phone is locked, the agent stops and asks
  you to unlock it. `safety.allow_unlock` is off by default. When it is on,
  the `agentphone run` terminal still does not enter a passcode: the agent asks
  you to unlock the phone by hand.
- **Take-over.** Push Ctrl+C at any time. The first Ctrl+C stops the agent, and
  you have control of the phone. A second Ctrl+C exits at once.
- **Step limit.** The run stops at `safety.max_steps` (default 50).

> CAUTION: Do not set `safety.confirm_irreversible` to `false`. The agent can
> then send messages or buy items without a question.

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
  app from `dumpsys window`. On an iPhone, agentphone knows only the app that
  the agent opened with `open_app`. After Home, the App Switcher, Lock, a swipe
  from the bottom edge, or a Cmd shortcut, the app is unknown.
- The app id contains a word from the blocklist, for example `bank`, `paypal`,
  `wallet`, `messages`, `whatsapp`, `gmail`, `password`, `authenticator`,
  `photos`, or `health` (see `DEFAULT_BLOCKLIST` in
  `crates/ap-jev/src/privacy.rs`).

The blocklist compares words with the app id. A banking app with an id that
contains no blocklist word is sent. On an iPhone, a tap that opens a different
app (a link, a share sheet) is not detected.


