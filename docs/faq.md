# FAQ

## Does agentphone work now?

The code builds and the unit tests pass on Windows. Nobody tested it on a real
phone yet. See the Status section in the [README](../README.md#status).

## Do I need a Mac for an iPhone?

No. agentphone works on Windows, macOS, and Linux. It needs no Xcode and no app
on the iPhone. Windows needs the Apple Devices app or iTunes.

## Which iPhones work?

agentphone connects to iPhones with iOS 17.4 or later. Touch input needs
iOS 27 or later, so use an iPhone 11 or newer with an iOS 27 release build.
Early iOS 27 betas list no HID service. The iPhone 13 is a main target.
Below iOS 27, the iPhone refuses the display stream with CoreDevice error
9021 (reported on iOS 18 and 26, expected on 17.x). agentphone then works in
screen only mode: screenshots, buttons, and keys can work, but tap and swipe
cannot, and `agentphone run` stops before it starts.

Models newer than `iPhone18,5` can need a Cryptex1 developer disk image for
input. agentphone does not support Cryptex1 yet, so input can fail on these
models. The disk image comes from a public mirror that agentphone does not
pin, so a change there can also change the result.

iPhone XS, XS Max, and XR stop at iOS 18. agentphone connects to them, but
touch input does not work, because these models cannot get iOS 27.

Run `agentphone doctor`. If the "Touch and keyboard input" check fails,
agentphone cannot control that iPhone. Below iOS 27, the check can pass and
input still fails.

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

With the default settings, agentphone asks you first. The model must ask
before each irreversible action. agentphone also asks before Enter, before
text with a newline, and before a tap right after typing in a messaging,
payment, or shopping app.

These checks lower the risk, but they do not remove it. Some depend on the
model, and some depend on what is on the screen. Push Ctrl+C to stop the agent
at any time. Ctrl+C is the hard stop. See
[agent.md](agent.md#safety-rules).

## Can the agent unlock my phone?

No. The agent never types a passcode. When the phone is locked, it asks you to
unlock it. agentphone also refuses 4 or more digits unless you approve that
exact call, and it refuses all text and keys while the phone looks locked. It
cannot find a password that has letters.

## Does agentphone send my screen anywhere?

It sends screenshots to the AI provider that you configure. With a local model
(Ollama, LM Studio), the screenshots stay on your computer. The optional Jev
feature also sends screenshots to typesafe.ai. Jev is off by default.

## What is the ESP32 dongle in the repository?

It is experimental code. It is not part of the product. See
[architecture.md](architecture.md#experimental-parts).
