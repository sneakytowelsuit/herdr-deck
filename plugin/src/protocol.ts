/**
 * The daemon protocol, mirrored from `crates/herdr-deck-core/src/protocol.rs`.
 *
 * These two definitions must agree. `FRONTEND_PROTOCOL` is the guard: the plugin sends it in
 * `hello`, and the daemon refuses a mismatch with an explicit message rather than failing in
 * some confusing way later. The plugin and the daemon install through completely different
 * mechanisms — the Elgato Marketplace and a release archive — so version skew is a genuine
 * possibility rather than a theoretical one.
 */

/** Bumped when this protocol changes incompatibly. Keep in step with the Rust constant. */
export const FRONTEND_PROTOCOL = 1;

/** Hardware geometry, reported once per connection. */
export interface DeviceReport {
	/** A model name the daemon recognises, when we can identify it. */
	model?: DeckModel | null;
	model_name?: string | null;
	columns: number;
	rows: number;
	key_image_px: number;
	dials: number;
	touchstrip?: { width: number; height: number } | null;
}

export type DeckModel = "original" | "mini" | "xl" | "plus" | "neo" | "pedal" | "unknown";

export type FrontendMessage =
	| { type: "hello"; frontend: string; device: DeviceReport; protocol: number }
	| { type: "key_down"; index: number }
	| { type: "key_up"; index: number }
	| { type: "dial_rotate"; dial: number; ticks: number }
	| { type: "dial_down"; dial: number }
	| { type: "dial_up"; dial: number }
	| { type: "touch_tap"; dial: number | null }
	| { type: "refresh" }
	| { type: "ping" };

export type DaemonMessage =
	| { type: "ready"; protocol: number; keys: number; dials: number; device: string }
	| { type: "set_key_image"; index: number; png: string }
	| { type: "set_dial_feedback"; dial: number; title: string; value: string; png?: string }
	| { type: "ok"; index: number }
	| { type: "alert"; index: number; message: string }
	| { type: "protocol_mismatch"; expected: number; got: number }
	| { type: "pong" };

/**
 * Split a stream of bytes into whole JSON lines.
 *
 * A socket read boundary lands wherever the kernel decides, so a message can arrive in pieces
 * or several can arrive together. Holding the remainder between calls is what makes the
 * transport reliable; parsing each chunk independently would silently drop messages under load,
 * which is exactly when the deck matters most.
 */
export class LineDecoder {
	private buffer = "";

	push(chunk: string): DaemonMessage[] {
		this.buffer += chunk;
		const messages: DaemonMessage[] = [];
		let newline = this.buffer.indexOf("\n");
		while (newline !== -1) {
			const line = this.buffer.slice(0, newline).trim();
			this.buffer = this.buffer.slice(newline + 1);
			if (line.length > 0) {
				const parsed = parseMessage(line);
				if (parsed) {
					messages.push(parsed);
				}
			}
			newline = this.buffer.indexOf("\n");
		}
		return messages;
	}

	/** Bytes held pending a newline. Exposed for tests. */
	get pending(): string {
		return this.buffer;
	}

	reset(): void {
		this.buffer = "";
	}
}

/** Parse one line, returning null rather than throwing — one bad line must not kill the link. */
export function parseMessage(line: string): DaemonMessage | null {
	try {
		const value = JSON.parse(line);
		if (value && typeof value === "object" && typeof value.type === "string") {
			return value as DaemonMessage;
		}
		return null;
	} catch {
		return null;
	}
}

export function encode(message: FrontendMessage): string {
	return `${JSON.stringify(message)}\n`;
}
