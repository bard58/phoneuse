# agentphone

Let an AI agent use your iPhone or Android phone from Windows, macOS or Linux.
One static Rust binary: `agentphone`.

Status: early rewrite. Full documentation is in progress.

## Build

```sh
cargo build --release -p ap-cli
./target/release/agentphone --help
```

## Origin and licence

agentphone is a Rust rewrite of [phoneuse](https://github.com/lxe/phoneuse) by
Aleksey Smolenchuk (MIT). The folder `legacy/python/` holds the original Python code.
This project also uses the MIT licence. See `LICENSE`.
