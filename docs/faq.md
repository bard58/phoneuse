# FAQ

## Does agentphone work now?

The code builds and the unit tests pass on Windows. Nobody tested it on a real
phone yet. See the Status section in the [README](../README.md#status).

## Do I need a Mac for an iPhone?

No. agentphone works on Windows, macOS, and Linux. It needs no Xcode and no app
on the iPhone. Windows needs the Apple Devices app or iTunes.

## Which iPhones work?

iPhones with iOS 17.4 or later. iPhone XS, XR, and newer models can run this
version. The iPhone 13 is a main target (iOS 17.4 to 26). Touch input on iOS
17.4 to 18 is less certain than on iOS 26.

## Why does an iPhone need Developer Mode?

Touch input and screenshots use Apple developer services. These services need
Developer Mode and the developer disk image.

## Why must I run setup again after an iPhone restart?

The iPhone removes the developer disk image at each restart. Run
`agentphone setup ios` to mount it again. This needs Internet.

## Do I need a cable all the time?

No. You need a cable for the first pairing. Then you can use Wi-Fi on the same
network. See [iphone.md](iphone.md) and [android.md](android.md).

## Which AI models can I use?

Claude (default `claude-opus-5-5`), or any OpenAI-compatible API, for example
OpenAI, OpenRouter, Ollama, or LM Studio. The model must accept images. See
[agent.md](agent.md).

## Where is my API key stored?

agentphone does not store it. It reads the key from an environment variable.

## Can the agent buy something or send a message without me?

Not with the default settings. The agent asks you before each irreversible
action. Push Ctrl+C to stop it at any time.

## Can the agent unlock my phone?

No. The agent never types a passcode. When the phone is locked, it asks you to
unlock it.

## Does agentphone send my screen anywhere?

It sends screenshots to the AI provider that you configure. With a local model
(Ollama, LM Studio), the screenshots stay on your computer. The optional Jev
feature also sends screenshots to typesafe.ai. Jev is off by default.

## What is the ESP32 dongle in the repository?

It is experimental code. It is not part of the product. See
[architecture.md](architecture.md#experimental-parts).
