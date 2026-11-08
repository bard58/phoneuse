# Troubleshooting

Run `agentphone doctor` first. It examines the config, the AI model, the
iPhone, Android, the connected phones, and OCR. For each problem, it prints one
"Fix:" line. Do the first fix, then run `agentphone doctor` again.

For machine-readable output, run `agentphone doctor --json`. For more log
lines, add `-v`, `-vv`, or `-vvv`.

The phone checks are in [iphone.md](iphone.md#troubleshooting) and
[android.md](android.md#troubleshooting).

## General problems

| Problem | Fix |
|---|---|
| "Config file" fails | Run `agentphone config edit` and correct the file. Or delete the file and run `agentphone setup`. |
| "Provider" fails | Run `agentphone config set llm.provider anthropic` (or `openai`). |
| "API key" warning | Set the environment variable that doctor names, then open a new terminal. MCP mode needs no key. |
| "Connected phones: none found" | Connect the phone, unlock it, and fix the problems above it in the doctor output. |
| "Default phone" warning | Run `agentphone devices`, then `agentphone use <id>`. |
| "OCR for tap-on" warning | OCR is optional. Build with `--features ocr`, then run `agentphone setup ocr`. |
| `setup` says it needs an interactive terminal | Run it in a normal terminal window, not from a script. |
| Windows: the key is not found after `setx` | Close the terminal and open a new one. `setx` changes only new terminals. |
| iPhone works on USB but not after a restart | The iPhone removed the DDI. Run `agentphone setup ios`. |
| The agent stops with "step limit" | Use `--max-steps` with a larger number. |
| MCP client shows no tools | Use the full path of `agentphone` in the client config. Restart the client. |
| `cargo install` fails with "requires rustc 1.88" or a let-chain error | Run `rustup update`. agentphone needs Rust 1.88. With `--features ocr`, it needs Rust 1.94. |
| "input may be partly delivered" | The connection dropped while the phone got the input. agentphone does not send it again. Look at the screen, then do the action again if necessary. |
| "may or may not have reached the phone" (MCP) | The same problem in MCP mode. The agent must call `look` before it tries again. |
| A time-out error on an iPhone | agentphone closed the session. The next command opens a new one. If it continues, unlock the iPhone and connect the cable again. |
| "OCR for tap-on" shows "not found" or "checksum mismatch" in an OCR build | A model file is missing, or its SHA-256 does not agree with the pinned value. Run `agentphone setup ocr`. |
| `--jev` says it needs a terminal | The consent question needs a terminal. Run the command in a terminal, or run it without `--jev`. |
| The agent refuses digits | 4 or more digits can be a passcode. Approve the exact call in the terminal, or type the digits yourself. |

## Report a problem

1. Run `agentphone doctor --json > doctor.json`.
2. Remove UDIDs and serial numbers from the file if you want to keep them
   private.
3. Open an issue at <https://github.com/bard58/phoneuse/issues> and attach the
   file.

> CAUTION: Do not attach the pairing files from the `ios-pairing` folder. They
> give control of your iPhone.
