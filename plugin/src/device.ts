/**
 * Turning what the Stream Deck app tells us into a {@link DeviceReport}.
 *
 * The daemon lays out the deck from geometry, not from a model name, so the job here is to
 * always produce something usable. A Stream Deck released after this code was written reports a
 * device type we do not recognise — and it must still work, from the columns and rows the app
 * gives us.
 *
 * # Dials are discovered, not tabulated
 *
 * We deliberately do not keep a table of "which models have how many dials". Two reasons, and
 * the second is the real one:
 *
 *  1. Elgato keeps shipping models (Studio, Plus XL); a table is wrong the day it is written.
 *  2. **We can only drive a control the user has placed our action on.** If someone puts our
 *     key action on a Stream Deck + but leaves the dials to another plugin, we have no business
 *     claiming four dials — the daemon would render touchstrip feedback nothing will ever draw.
 *
 * So the dial count comes from how many dial actions have actually appeared.
 */

import type { DeckModel, DeviceReport } from "./protocol.js";

/**
 * Elgato's numeric device types, as of SDK 2.x.
 *
 * Only used for a friendly name and a native key size. Anything missing here still works.
 */
export const DEVICE_TYPE: Record<number, DeckModel> = {
	0: "original",
	1: "mini",
	2: "xl",
	5: "pedal",
	7: "plus",
	9: "neo",
};

/**
 * Native key image size per model, in pixels.
 *
 * The SDK reports how many keys there are but not how large their images should be, so this
 * fills the gap. Unknown hardware falls back to 96px, which every current deck scales
 * acceptably — a slightly-wrong size is a soft failure; a missing one is a blank key.
 */
const KEY_IMAGE_PX: Partial<Record<DeckModel, number>> = {
	original: 72,
	mini: 80,
	xl: 96,
	plus: 120,
	neo: 96,
	pedal: 0,
};

const DEFAULT_KEY_IMAGE_PX = 96;

/**
 * The touchstrip above the dials.
 *
 * 800x100 on the Stream Deck +. Only reported when dials were actually observed, so hardware
 * with encoders but no strip simply gets no strip feedback.
 */
const TOUCHSTRIP = { width: 800, height: 100 };

/** The shape the SDK hands us for a connected device. */
export interface SdkDeviceInfo {
	type?: number;
	name?: string;
	size?: { columns?: number; rows?: number };
}

/**
 * Build a report.
 *
 * @param info What the SDK knows about the device.
 * @param observedDials How many dial actions have appeared. See the note above on why this is
 *   discovered rather than looked up.
 */
export function describeDevice(info: SdkDeviceInfo, observedDials = 0): DeviceReport {
	const model = info.type !== undefined ? DEVICE_TYPE[info.type] : undefined;
	const columns = info.size?.columns ?? fallbackColumns(model);
	const rows = info.size?.rows ?? fallbackRows(model);

	return {
		model: model ?? "unknown",
		model_name: info.name ?? (model ? prettyName(model) : "Stream Deck"),
		columns,
		rows,
		key_image_px: model ? (KEY_IMAGE_PX[model] ?? DEFAULT_KEY_IMAGE_PX) : DEFAULT_KEY_IMAGE_PX,
		dials: observedDials,
		touchstrip: observedDials > 0 ? TOUCHSTRIP : null,
	};
}

function fallbackColumns(model: DeckModel | undefined): number {
	switch (model) {
		case "mini":
			return 3;
		case "xl":
			return 8;
		case "plus":
		case "neo":
			return 4;
		case "pedal":
			return 3;
		default:
			return 5;
	}
}

function fallbackRows(model: DeckModel | undefined): number {
	switch (model) {
		case "mini":
		case "plus":
		case "neo":
			return 2;
		case "xl":
			return 4;
		case "pedal":
			return 1;
		default:
			return 3;
	}
}

function prettyName(model: DeckModel): string {
	switch (model) {
		case "original":
			return "Stream Deck";
		case "mini":
			return "Stream Deck Mini";
		case "xl":
			return "Stream Deck XL";
		case "plus":
			return "Stream Deck +";
		case "neo":
			return "Stream Deck Neo";
		case "pedal":
			return "Stream Deck Pedal";
		default:
			return "Stream Deck";
	}
}

/**
 * Have the reported capabilities changed in a way the daemon needs to know about?
 *
 * Used to decide whether to re-announce. Re-sending an identical report on every key appearance
 * would make the daemon rebuild its layout — and repaint the deck — dozens of times at startup.
 */
export function reportsDiffer(a: DeviceReport | null, b: DeviceReport): boolean {
	if (!a) {
		return true;
	}
	return (
		a.columns !== b.columns ||
		a.rows !== b.rows ||
		a.key_image_px !== b.key_image_px ||
		a.dials !== b.dials ||
		a.model !== b.model
	);
}

/**
 * Map a coordinate to a flat key index, the way the daemon numbers keys.
 *
 * The SDK addresses keys by (column, row); the daemon uses a single index in reading order.
 * Getting this backwards would scramble the whole deck, so it lives in one place with a test.
 */
export function keyIndex(column: number, row: number, columns: number): number {
	return row * columns + column;
}
