# MCP server

`agentphone mcp` starts an MCP (Model Context Protocol) server. An MCP client,
for example Claude Code, Claude Desktop, or Cursor, uses it to see and operate
the phone. The client supplies the AI model, so the MCP server needs no API key.

By default, the server uses stdin and stdout. Logs go to stderr.

Before you start, set up the phone. See [iphone.md](iphone.md) or
[android.md](android.md).

## Claude Code

```sh
claude mcp add --scope user agentphone -- agentphone mcp
```

`--scope user` makes agentphone available in all projects. If `agentphone` is
not on `PATH`, give the full path:

```powershell
claude mcp add --scope user agentphone -- C:\Tools\agentphone.exe mcp
```

`agentphone setup mcp` runs this command for you when `claude` is on `PATH`.

## Claude Desktop

1. Open the configuration file:
   - Windows: `%APPDATA%\Claude\claude_desktop_config.json`
   - macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
2. Add the `agentphone` server:

   ```json
   {
     "mcpServers": {
       "agentphone": {
         "command": "C:\\Tools\\agentphone.exe",
         "args": ["mcp"]
       }
     }
   }
   ```

   On macOS, use the full path of the binary, for example
   `/Users/you/.cargo/bin/agentphone`.
3. Restart Claude Desktop.

## Cursor

1. Open `%USERPROFILE%\.cursor\mcp.json` (all projects) or `.cursor\mcp.json`
   in the project. On macOS and Linux, the file is `~/.cursor/mcp.json`.
2. Add the server:

   ```json
   {
     "mcpServers": {
       "agentphone": {
         "command": "agentphone",
         "args": ["mcp"]
       }
     }
   }
   ```

## HTTP mode

The server can also use streamable HTTP on this computer.

1. Start the server:

   ```sh
   agentphone mcp --http
   ```

   The endpoint is `http://127.0.0.1:8778/mcp`. To use a different port, give
   the address: `agentphone mcp --http 127.0.0.1:9000`.
2. Add it to the client:

   ```sh
   claude mcp add --transport http agentphone http://127.0.0.1:8778/mcp
   ```

The server listens only on a loopback address. It refuses other addresses, and
it refuses Host headers other than `localhost`, `127.0.0.1`, and `::1`.

## Select a phone

The server uses the default phone (`agentphone use <id>`), or the only connected
phone. To set the phone for one client, add `--device <id>` before `mcp`, or set
the environment variable `AGENTPHONE_DEVICE`.

## Tools

All coordinates are fractions of the screen from 0.0 to 1.0, from the top-left
corner. The center is (0.5, 0.5).

| Tool | What it does |
|---|---|
| `look` | Shows the screen: `both` (default, image and text), `strip`, `text`, or `image`. `grid=true` draws a 0.1 grid. |
| `wait` | Waits 0 to 10 seconds, then looks again. |
| `tap` | Taps at (x, y). `double=true` taps two times. |
| `tap_on` | Taps visible text. Needs OCR. |
| `long_press` | Touches and holds at (x, y). |
| `swipe` | Drags from (x0, y0) to (x1, y1). |
| `scroll` | Scrolls up, down, left, or right by half-screens. |
| `type_text` | Types into the focused field. `\n` pushes Enter. Replaces the old text unless `replace=false`. |
| `press_key` | Pushes a key or shortcut: `enter`, `backspace`, `cmd+a`. |
| `button` | `home`, `lock`, `volume_up`, `volume_down`, `app_switcher`, `back`. |
| `open_app` | Opens an app by name, bundle id, or package name. |
| `navigate` | `home`, `back`, `app_switcher`, `control_center`, `notifications`. |
| `list_apps` | Lists installed app ids, when the backend can. |
| `ui_tree` | Shows the UI hierarchy, when the backend has one. |
| `end_session` | Releases the phone. The next call connects again. |
| `status` | Shows the device, the capabilities, the OCR state, and setup errors with a fix. |

Each action returns the screen in the same call:

1. One JPEG image with frames at +0.1, +0.4, and +0.8 seconds after the action.
   The server drops frames that did not change.
2. One text block with the result, the strip legend, and the text lines of the
   last frame (when OCR is available).

There is no `unlock` tool. The server tells the agent to ask you to unlock the
phone. The agent must never type a passcode.

## OCR

`tap_on` and the text lines need OCR.

1. Build with the `ocr` feature:

   ```sh
   cargo install --git https://github.com/bard58/phoneuse --branch rust-rewrite ap-cli --features ocr
   ```

2. Download the models (about 12 MB):

   ```sh
   agentphone setup ocr
   ```

Without OCR, actions return only the image. `tap_on` returns an error that tells
the agent to use `tap(x, y)`.
