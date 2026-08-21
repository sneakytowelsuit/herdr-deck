import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

// The touchstrip is drawn by the daemon and shipped as a PNG, exactly like the keys. The plugin
// once ignored that PNG and set only title/value, so a real Stream Deck + showed the stock
// encoder layout — four red dots from our own icon — and nothing about herdr. Nothing caught it,
// because the invariant spans three files: the manifest names a layout, the layout names an item,
// and plugin.ts sets that item. These tests hold the three together.

const dir = join(import.meta.dirname, "..", "com.sneakytowelsuit.herdr-deck.sdPlugin");
const manifest = JSON.parse(readFileSync(join(dir, "manifest.json"), "utf8"));
const dialAction = manifest.Actions.find((a: { UUID: string }) => a.UUID.endsWith(".dial"));

test("the dial action declares a layout, or the daemon's tile has nowhere to go", () => {
	assert.ok(dialAction, "the plugin defines a dial action");
	assert.ok(
		dialAction.Encoder?.layout,
		"without an Encoder.layout the touchstrip falls back to the stock one",
	);
});

test("the layout is a single full-bleed pixmap the size the daemon renders", () => {
	const layout = JSON.parse(readFileSync(join(dir, dialAction.Encoder.layout), "utf8"));
	const pixmaps = layout.items.filter((i: { type: string }) => i.type === "pixmap");
	assert.equal(pixmaps.length, 1, "one image, so what the daemon draws is what appears");
	assert.deepEqual(
		pixmaps[0].rect,
		[0, 0, 200, 100],
		"a Stream Deck + touchstrip segment is 200x100, which is what herdr-deckd renders",
	);
});

test("the plugin sets the item the layout actually declares", () => {
	const layout = JSON.parse(readFileSync(join(dir, dialAction.Encoder.layout), "utf8"));
	const key = layout.items.find((i: { type: string }) => i.type === "pixmap").key;
	const source = readFileSync(join(import.meta.dirname, "..", "src", "plugin.ts"), "utf8");
	assert.ok(
		source.includes(`${key}: \`data:image/png;base64,`),
		`plugin.ts must feed the daemon's PNG to \`${key}\`; a renamed item silently draws nothing`,
	);
});
