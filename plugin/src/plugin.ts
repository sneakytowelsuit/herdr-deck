/**
 * The Stream Deck plugin entry point (macOS).
 *
 * This file is deliberately thin. It does four things and nothing else:
 *
 *  1. tells the daemon what hardware is attached and which controls we can actually drive,
 *  2. forwards key and dial events,
 *  3. draws the PNGs the daemon sends back,
 *  4. reconnects when the daemon restarts.
 *
 * Every decision about what a key *means* lives in `herdr-deckd`. That is what keeps macOS and
 * Linux behaving identically instead of drifting apart one small fix at a time.
 */

import streamDeck, {
	action,
	SingletonAction,
	type DialAction,
	type DialDownEvent,
	type DialRotateEvent,
	type DialUpEvent,
	type KeyAction,
	type KeyDownEvent,
	type KeyUpEvent,
	type TouchTapEvent,
	type WillAppearEvent,
	type WillDisappearEvent,
} from "@elgato/streamdeck";

import { DaemonClient } from "./client.js";
import { describeDevice, keyIndex, reportsDiffer, type SdkDeviceInfo } from "./device.js";
import type { DaemonMessage, DeviceReport } from "./protocol.js";

const client = new DaemonClient();

/** Visible key actions, by flat key index. */
const keys = new Map<number, KeyAction>();
/** Visible dial actions, by dial index. */
const dials = new Map<number, DialAction>();

/** Columns on the attached deck, needed to flatten (column, row) into an index. */
let columns = 5;
/** The last report we sent, so we only re-announce on a real change. */
let announced: DeviceReport | null = null;
/** Debounce handle: actions appear in a burst at startup. */
let announceTimer: NodeJS.Timeout | null = null;

/**
 * Announce the hardware, coalescing the burst of appearances at startup.
 *
 * Actions appear one at a time when a profile loads. Announcing on each would make the daemon
 * rebuild its layout and repaint the whole deck a dozen times in a row.
 */
function scheduleAnnounce(): void {
	if (announceTimer) {
		return;
	}
	announceTimer = setTimeout(() => {
		announceTimer = null;
		announceDevice();
	}, 250);
	announceTimer.unref?.();
}

function announceDevice(): void {
	const device = [...streamDeck.devices].find((d) => d.isConnected) ?? [...streamDeck.devices][0];
	const info: SdkDeviceInfo = device
		? {
				type: device.type as unknown as number,
				name: device.name,
				size: device.size
					? { columns: device.size.columns, rows: device.size.rows }
					: undefined,
			}
		: {};

	// Dials we can actually drive — see the note in device.ts on why this is discovered.
	const observedDials = dials.size === 0 ? 0 : Math.max(...dials.keys()) + 1;
	const report = describeDevice(info, observedDials);

	if (!reportsDiffer(announced, report)) {
		return;
	}
	announced = report;
	columns = report.columns;
	streamDeck.logger.info(
		`deck: ${report.model_name} ${report.columns}x${report.rows}, ${report.dials} dial(s)`,
	);
	client.setDevice(report);
}


/**
 * Pull physical coordinates out of an event payload.
 *
 * An action placed inside a Multi Action has no coordinates — it is not on a physical key at
 * all. Those instances are skipped: there is no key for the daemon to paint, and nothing
 * sensible to map a press to.
 */
function coordsOf(payload: unknown): { column: number; row: number } | undefined {
	if (payload && typeof payload === "object" && "coordinates" in payload) {
		return (payload as { coordinates?: { column: number; row: number } }).coordinates ?? undefined;
	}
	return undefined;
}

/** Draw what the daemon tells us to draw. */
function applyDaemonMessage(message: DaemonMessage): void {
	switch (message.type) {
		case "ready":
			streamDeck.logger.info(
				`daemon ready: ${message.device} (${message.keys} keys, ${message.dials} dials)`,
			);
			break;

		case "set_key_image": {
			// A key we are not currently showing is normal — the user may have placed our action
			// on only part of the deck. Ignoring it is correct.
			void keys.get(message.index)?.setImage(`data:image/png;base64,${message.png}`);
			break;
		}

		case "set_dial_feedback": {
			void dials.get(message.dial)?.setFeedback({
				title: message.title,
				value: message.value,
			});
			break;
		}

		case "ok":
			void keys.get(message.index)?.showOk();
			break;

		case "alert":
			// A partial success — herdr focused but the window did not come forward — surfaces
			// here, so the user is never left pressing a key that appears to do nothing.
			streamDeck.logger.warn(`key ${message.index}: ${message.message}`);
			void keys.get(message.index)?.showAlert();
			break;

		case "protocol_mismatch":
			streamDeck.logger.error(
				`herdr-deckd speaks frontend protocol ${message.expected}, this plugin speaks ` +
					`${message.got}. Update whichever is older.`,
			);
			break;

		case "pong":
			break;
	}
}

client.on("message", applyDaemonMessage);
client.on("connected", () => {
	streamDeck.logger.info("connected to herdr-deckd");
	// A fresh daemon knows nothing about us and only sends what changed, so ask for everything.
	client.send({ type: "refresh" });
});
client.on("disconnected", (reason: string) =>
	streamDeck.logger.debug(`herdr-deckd disconnected: ${reason}`),
);

/**
 * A key showing an agent, or one of the fixed controls.
 *
 * One action type covers every key: the daemon decides what each index means, so there is no
 * reason to make the user pick between half a dozen near-identical actions. Drop this on every
 * key and the deck configures itself.
 */
@action({ UUID: "com.sneakytowelsuit.herdr-deck.key" })
export class HerdrKey extends SingletonAction {
	override onWillAppear(event: WillAppearEvent): void {
		const coordinates = coordsOf(event.payload);
		if (!coordinates || !event.action.isKey()) {
			return;
		}
		keys.set(keyIndex(coordinates.column, coordinates.row, columns), event.action);
		scheduleAnnounce();
		// This key has no image yet and the daemon only sends what changed.
		client.send({ type: "refresh" });
	}

	override onWillDisappear(event: WillDisappearEvent): void {
		const coordinates = coordsOf(event.payload);
		if (coordinates) {
			keys.delete(keyIndex(coordinates.column, coordinates.row, columns));
		}
	}

	// Both edges are forwarded and neither is interpreted here. The daemon times the gap between
	// them, because how long a key was held is part of what the press *means* — and meaning is
	// not something a frontend is allowed to decide.
	override onKeyDown(event: KeyDownEvent): void {
		const coordinates = coordsOf(event.payload);
		if (coordinates) {
			client.send({
				type: "key_down",
				index: keyIndex(coordinates.column, coordinates.row, columns),
			});
		}
	}

	override onKeyUp(event: KeyUpEvent): void {
		const coordinates = coordsOf(event.payload);
		if (coordinates) {
			client.send({
				type: "key_up",
				index: keyIndex(coordinates.column, coordinates.row, columns),
			});
		}
	}
}

/**
 * A dial on hardware that has encoders.
 *
 * Rotate scrubs a list, press focuses the selection, and the touchstrip above shows where you
 * are. Which list each dial scrubs is the daemon's decision.
 */
@action({ UUID: "com.sneakytowelsuit.herdr-deck.dial" })
export class HerdrDial extends SingletonAction {
	override onWillAppear(event: WillAppearEvent): void {
		const coordinates = coordsOf(event.payload);
		if (!coordinates || !event.action.isDial()) {
			return;
		}
		dials.set(coordinates.column, event.action);
		// Seeing a dial is how the daemon learns this deck has encoders at all.
		scheduleAnnounce();
		client.send({ type: "refresh" });
	}

	override onWillDisappear(event: WillDisappearEvent): void {
		const coordinates = coordsOf(event.payload);
		if (coordinates) {
			dials.delete(coordinates.column);
			scheduleAnnounce();
		}
	}

	override onDialRotate(event: DialRotateEvent): void {
		const coordinates = coordsOf(event.payload);
		if (coordinates) {
			client.send({
				type: "dial_rotate",
				dial: coordinates.column,
				ticks: event.payload.ticks,
			});
		}
	}

	override onDialDown(event: DialDownEvent): void {
		const coordinates = coordsOf(event.payload);
		if (coordinates) {
			client.send({ type: "dial_down", dial: coordinates.column });
		}
	}

	override onDialUp(event: DialUpEvent): void {
		const coordinates = coordsOf(event.payload);
		if (coordinates) {
			client.send({ type: "dial_up", dial: coordinates.column });
		}
	}

	/** Tapping the strip above a dial does what pressing that dial does. */
	override onTouchTap(event: TouchTapEvent): void {
		const coordinates = coordsOf(event.payload);
		client.send({ type: "touch_tap", dial: coordinates?.column ?? null });
	}
}

streamDeck.actions.registerAction(new HerdrKey());
streamDeck.actions.registerAction(new HerdrDial());

streamDeck.devices.onDeviceDidConnect(() => scheduleAnnounce());
streamDeck.devices.onDeviceDidDisconnect(() => streamDeck.logger.info("deck disconnected"));

void streamDeck.connect().then(() => {
	announceDevice();
	client.connect();
});
