# Contributing

## Layout

| Path | What |
|---|---|
| `crates/herdr-deck-herdr` | herdr wire protocol — and nothing else. |
| `crates/herdr-deck-core` | State, layout, rendering, config, frontend protocol. |
| `crates/herdr-deck-focus` | herdr focus plus OS window raising. |
| `crates/herdr-deck-hid` | Linux USB HID frontend. |
| `crates/herdr-deckd` | The daemon. |
| `crates/herdr-deck-cli` | The `herdr-deck` tool. |
| `plugin/` | The macOS Stream Deck plugin (TypeScript). |
| `docs/` | This site. |

## Building and testing

```sh
cargo test                     # everything; no herdr and no hardware needed
cargo clippy --all-targets
cargo fmt

cd plugin && npm install && npm test && npm run build
```

On Linux the HID frontend needs `libudev-dev` (`pkg-config` too).

## The rules worth knowing

**All herdr protocol knowledge stays in `herdr-deck-herdr`.** No production code above it should
construct a herdr method name or match on a herdr JSON field. That is what makes a herdr
protocol change a one-crate fix.

Tests are the one place this needs qualifying, because standing up a fake herdr inevitably
touches its surface. The rule there is: build fixtures from the **typed wire structs**
(`AgentInfo`, `SessionSnapshot`) rather than hand-written JSON, and reach for an intent-level
helper on `MockHerdr` (`serve_session`, `agent_focus_targets`, …) rather than naming a method.
When no helper fits, add one to `herdr-deck-herdr::mock` — that keeps new protocol knowledge
accumulating in the crate that owns it. Some older tests still call `mock.reply("…")` with a
raw method name; those are worth converting when you are next in the file, not worth a
dedicated sweep.

**Frontends stay dumb.** If you find yourself deciding what a key *means* in the plugin or the
HID driver, it belongs in the daemon. Anything else and the two platforms drift apart one small
fix at a time.

**Status is never colour alone.** Every agent state has a distinct glyph as well as a distinct
colour. If you add a state, give it a shape.

**Degrade loudly.** When herdr-deck cannot do what was asked — raise a window, reach a tool — it
says so on the key and in `doctor`. Silently doing less than the user asked for is the one thing
to avoid.

## Testing without hardware

You do not need a Stream Deck or a running herdr:

- `herdr-deck-herdr` has a fake herdr server behind its `mock` feature, reproducing the two
  behaviours that bite: one-shot RPC connections and long-lived subscriptions.
- `herdr-deck-focus` takes a command runner, so backends are tested with no desktop.
- The daemon's session is a pure state machine — messages in, messages out.
- `herdr-deckd --dry-run DIR` renders every tile to disk.

## Changing the theme

Regenerate the plugin icons so they stay in step:

```sh
cargo run -p herdr-deck-cli -- icons --out plugin/com.sneakytowelsuit.herdr-deck.sdPlugin/imgs
```

`crates/herdr-deck-core/tests/golden.rs` pins what tiles actually look like — colours, glyphs,
text wrapping, the focus ring — against PNG fixtures committed under
`crates/herdr-deck-core/tests/golden/`. Any theme, layout, or font change will fail those tests;
that is the point, not a bug. Once you have confirmed the new look is what you meant, regenerate
the fixtures and eyeball the diff (with an image viewer, not `git diff --stat`) before committing:

```sh
UPDATE_GOLDEN=1 cargo test --offline -p herdr-deck-core --test golden
```

## Re-checking the herdr protocol

herdr-deck targets socket protocol 19. To see what a given herdr speaks:

```sh
herdr api schema --json | jq '{protocol, schema_version}'
```

If it has moved, the changes belong in `crates/herdr-deck-herdr`, and
[herdr protocol notes](../reference/herdr-protocol.md) should be updated alongside.

## Validating the plugin

```sh
cd plugin
npx @elgato/cli validate com.sneakytowelsuit.herdr-deck.sdPlugin
```
