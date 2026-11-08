# ap-mcp

The agentphone MCP server. An external agent (Claude Code, Claude Desktop,
Cursor, or any MCP client) uses it to see and operate a real phone. It is a
port of the phoneuse `mcp_server.py` tool design.

## Tools

All coordinates are normalized: `x` and `y` are fractions of the screen from
0.0 to 1.0, measured from the top-left corner. The centre is (0.5, 0.5).

| Tool | What it does |
|---|---|
| `look` | The screen now: `both` (default, image + text), `strip`, `text`, `image`. `grid=true` draws a 0.1 grid. |
| `wait` | Wait 0 to 10 s, then look again. |
| `tap` | Tap at (x, y). `double=true` taps twice. |
| `tap_on` | Tap visible text by what it says. Needs OCR. |
| `long_press` | Press and hold at (x, y). |
| `swipe` | Drag from (x0, y0) to (x1, y1). |
| `scroll` | Scroll the content up, down, left or right by half-screens. |
| `type_text` | Type into the focused field. `\n` presses Enter. Replaces the old text unless `replace=false`. |
| `press_key` | A key or shortcut: `enter`, `backspace`, `cmd+a`, ... |
| `button` | `home`, `lock`, `volume_up`, `volume_down`, `app_switcher`. |
| `open_app` | Open an app by name or bundle id / package name. |
| `navigate` | `home`, `back`, `app_switcher`, `control_center`, `notifications`. |
| `list_apps` | Installed app ids, when the backend can list them. |
| `ui_tree` | The UI hierarchy, when the backend has one. |
| `end_session` | Release the phone. The next call connects again. |
| `status` | Device, backend capabilities, OCR state, and setup errors with a fix. |

Every action returns the screen in the same call:

1. One JPEG image: frames at +0.1, +0.4 and +0.8 s after the action, side by
   side. Frames that did not change are dropped, so a still screen is one frame.
2. One text block: the result of the action, the strip legend, and, when OCR is
   available, the text lines of the last frame as `x y  text`.

There is no `unlock` tool. The server tells the agent to ask the user to unlock
the phone, and never to type a passcode.

## Client configuration

The `agentphone` binary starts the server with `agentphone mcp`. The server
speaks MCP on stdin and stdout. Logs go to stderr.

### Claude Code

```bash
claude mcp add agentphone -- agentphone mcp
```

Add `--scope user` to use it in all projects. If `agentphone` is not on `PATH`,
give the full path, for example
`claude mcp add agentphone -- C:\Tools\agentphone.exe mcp`.

### Claude Desktop (Windows)

Edit `%APPDATA%\Claude\claude_desktop_config.json`. Then restart Claude Desktop.

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

On macOS the file is `~/Library/Application Support/Claude/claude_desktop_config.json`.

### Cursor

Edit `%USERPROFILE%\.cursor\mcp.json` (all projects) or `.cursor\mcp.json` in
the project.

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

### Streamable HTTP

With the `http` feature (on by default), the server can also listen on a
loopback address. The endpoint is `http://127.0.0.1:8778/mcp`. The server
refuses other listen addresses and Host headers other than localhost,
127.0.0.1 and ::1.

```bash
claude mcp add --transport http agentphone http://127.0.0.1:8778/mcp
```

## OCR

Text lines and `tap_on` need OCR. Build with the `ocr` feature, then download
the models with `agentphone setup ocr`. Without OCR, actions return only the
image and `tap_on` returns an error that tells the agent to use `tap(x, y)`.

## Using the crate

```rust
use ap_mcp::{PhoneServer, ProviderSource, ServerOptions, serve_stdio};

let source = ProviderSource::new(providers, config.default_device.clone());
let server = PhoneServer::with_options(source, ServerOptions::default());
serve_stdio(server).await?;
// or: ap_mcp::serve_http(server, ap_mcp::DEFAULT_HTTP_ADDR.parse()?).await?;
```

`DeviceSource` is the only link to a backend. The server connects on the first
tool call, drops the device when the connection is lost, and connects again on
the next call. `Error::setup` hints from the source reach the agent as text.
