import assert from "node:assert/strict";
import { test } from "node:test";

import { encode, FRONTEND_PROTOCOL, LineDecoder, parseMessage } from "./protocol.js";
import { describeDevice, keyIndex, reportsDiffer, type SdkDeviceInfo } from "./device.js";

test("a message split across socket reads is reassembled", () => {
	// Read boundaries land wherever the kernel decides. Dropping a half-arrived message would
	// leave a key showing stale status precisely when things are busy.
	const decoder = new LineDecoder();
	assert.deepEqual(decoder.push('{"type":"po'), []);
	assert.deepEqual(decoder.push('ng"}\n'), [{ type: "pong" }]);
});

test("several messages in one read are all delivered", () => {
	const decoder = new LineDecoder();
	const messages = decoder.push('{"type":"pong"}\n{"type":"ok","index":3}\n');
	assert.equal(messages.length, 2);
	assert.deepEqual(messages[1], { type: "ok", index: 3 });
});

test("a partial trailing message is held, not dropped", () => {
	const decoder = new LineDecoder();
	const messages = decoder.push('{"type":"pong"}\n{"type":"ok"');
	assert.equal(messages.length, 1);
	assert.equal(decoder.pending, '{"type":"ok"');
});

test("a malformed line is skipped without killing the stream", () => {
	const decoder = new LineDecoder();
	const messages = decoder.push('not json\n{"type":"pong"}\n');
	assert.deepEqual(messages, [{ type: "pong" }]);
});

test("blank lines are ignored", () => {
	const decoder = new LineDecoder();
	assert.deepEqual(decoder.push('\n\n{"type":"pong"}\n'), [{ type: "pong" }]);
});

test("reset clears buffered bytes so a reconnect cannot resume mid-message", () => {
	const decoder = new LineDecoder();
	decoder.push('{"type":"po');
	decoder.reset();
	assert.equal(decoder.pending, "");
	assert.deepEqual(decoder.push('{"type":"pong"}\n'), [{ type: "pong" }]);
});

test("a JSON value that is not a message is rejected", () => {
	assert.equal(parseMessage("[1,2,3]"), null);
	assert.equal(parseMessage('{"no_type":true}'), null);
	assert.equal(parseMessage("null"), null);
});

test("encoded messages are exactly one line", () => {
	const line = encode({ type: "key_down", index: 4 });
	assert.ok(line.endsWith("\n"));
	assert.equal(line.split("\n").length, 2);
	assert.deepEqual(JSON.parse(line.trim()), { type: "key_down", index: 4 });
});

test("a Stream Deck + is described with its dials and touchstrip once they are observed", () => {
	const info: SdkDeviceInfo = { type: 7, name: "Stream Deck +", size: { columns: 4, rows: 2 } };
	const report = describeDevice(info, 4);
	assert.equal(report.model, "plus");
	assert.equal(report.dials, 4);
	assert.equal(report.key_image_px, 120);
	assert.deepEqual(report.touchstrip, { width: 800, height: 100 });
});

test("dials are only claimed when our actions are actually on them", () => {
	// A Stream Deck + whose dials belong to another plugin: claiming four would make the daemon
	// render touchstrip feedback that nothing will ever draw.
	const report = describeDevice({ type: 7, size: { columns: 4, rows: 2 } }, 0);
	assert.equal(report.dials, 0);
	assert.equal(report.touchstrip, null);
});

test("a deck without dials reports none, so no scrub lands on a missing control", () => {
	for (const [type, model] of [
		[0, "original"],
		[1, "mini"],
		[2, "xl"],
		[9, "neo"],
	] as const) {
		const report = describeDevice({ type, size: { columns: 5, rows: 3 } });
		assert.equal(report.model, model);
		assert.equal(report.dials, 0, `${model} should report no dials`);
		assert.equal(report.touchstrip, null);
	}
});

test("hardware newer than the type table still reports dials it can drive", () => {
	// Studio and Plus XL are not in the table; discovery covers them anyway.
	const report = describeDevice({ type: 13, name: "Stream Deck + XL", size: { columns: 8, rows: 4 } }, 4);
	assert.equal(report.model, "unknown");
	assert.equal(report.dials, 4);
	assert.equal(report.columns, 8);
});

test("re-announcing an identical report is suppressed", () => {
	// Otherwise every key appearing at startup would rebuild the daemon layout and repaint.
	const first = describeDevice({ type: 7, size: { columns: 4, rows: 2 } }, 4);
	const same = describeDevice({ type: 7, size: { columns: 4, rows: 2 } }, 4);
	assert.equal(reportsDiffer(first, same), false);
	assert.equal(reportsDiffer(null, first), true, "the first report always announces");
});

test("discovering a dial counts as a real change", () => {
	const before = describeDevice({ type: 7, size: { columns: 4, rows: 2 } }, 0);
	const after = describeDevice({ type: 7, size: { columns: 4, rows: 2 } }, 4);
	assert.equal(reportsDiffer(before, after), true);
});

test("hardware newer than this code still produces a usable report", () => {
	// The whole point of geometry-driven layout: an unrecognised device type must not brick the
	// plugin.
	const report = describeDevice({ type: 99, name: "Stream Deck 9000", size: { columns: 6, rows: 3 } });
	assert.equal(report.model, "unknown");
	assert.equal(report.model_name, "Stream Deck 9000");
	assert.equal(report.columns, 6);
	assert.equal(report.rows, 3);
	assert.ok(report.key_image_px > 0, "must still have a drawable key size");
});

test("a device that reports no size falls back to the model's known geometry", () => {
	const report = describeDevice({ type: 2 });
	assert.equal(report.columns, 8);
	assert.equal(report.rows, 4);
});

test("a device with neither type nor size still yields something drawable", () => {
	const report = describeDevice({});
	assert.ok(report.columns > 0 && report.rows > 0);
	assert.ok(report.key_image_px > 0);
});

test("key coordinates flatten in reading order, matching the daemon", () => {
	// Getting this backwards would scramble the entire deck.
	assert.equal(keyIndex(0, 0, 4), 0);
	assert.equal(keyIndex(3, 0, 4), 3);
	assert.equal(keyIndex(0, 1, 4), 4);
	assert.equal(keyIndex(3, 1, 4), 7);
	assert.equal(keyIndex(7, 3, 8), 31);
});

test("the protocol constant is a number the daemon can compare", () => {
	assert.equal(typeof FRONTEND_PROTOCOL, "number");
	assert.ok(FRONTEND_PROTOCOL >= 1);
});
