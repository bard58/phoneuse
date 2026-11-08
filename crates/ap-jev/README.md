# ap-jev

Typed screen judgments for agentphone from Jev, the TypeSafe System One model.
Jev does not write text. It returns probabilities and typed answers
(Noul, Choice, Score, Box) for questions about a state.

This crate is optional. agentphone works without it.

## What it does

- `JevClient`: HTTP client for `POST https://api.typesafe.ai/v1/systemone`.
  It retries on 429, 529, 5xx and network errors with backoff.
- `ScreenJudge::judge(frame, last_action)`: one batched request that returns
  `settled`, `locked`, `keyboard_visible`, `alert_visible`, `sensitive` and
  `action_irreversible`.
- `ScreenJudge::is_irreversible(frame, action)`: probability for the
  confirm-before-irreversible gate.
- `ScreenJudge::pick_target(input, goal)`: picks one text candidate (with an
  optional screenshot). When `accepted` is false, use the main LLM.
- `ScreenJudge::locate(frame, description)` (experimental): Box plus a
  presence check. Returns a normalized point or `None`.

Screenshots are resized so that each side is 280 to 1568 px, then sent as JPEG.

## Cost and speed

- About $0.042 per 1M input tokens. Output tokens are free.
- About 250 ms for one batched request (TypeSafe cookbook figure).
- Measure on your own screens. Image requests use more input tokens.

## Privacy warning

Every call sends the screen (and text from it) to TypeSafe, a third-party
service. Image support is a "danger zone" feature of the API and can change.

- `ScreenJudge` refuses every call with `Error::Refused` until you call
  `privacy().grant_consent()` for the run.
- It also refuses when the foreground app matches the blocklist: banking and
  payments, Wallet, Messages and chat apps, mail, password managers and
  authenticators, Photos, Health. Edit the list with `PrivacyConfig`
  (`blocklist`, `allow`, `block_unknown_app`).
- Call `privacy().set_foreground_app(..)` when the app changes. An unknown
  app passes unless `block_unknown_app` is true.
- `JevClient` applies no privacy rules. Do not send screens through it directly.

## Enable it

1. Get an API key from <https://typesafe.ai>.
2. Set `TYPESAFE_API_KEY`. Optional: `TYPESAFE_MODEL` (default `jev-latest`;
   Box needs Jev 1.14 or later) and `TYPESAFE_BASE_URL`.
3. Build the CLI with the `jev` feature: `cargo build -p ap-cli --features jev`.
   The feature is off by default.
