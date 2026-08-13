/**
 * The daemon connection.
 *
 * A unix socket rather than a TCP port: nothing needs to reach this off-box, and filesystem
 * permissions already express exactly the access we want. Node speaks unix sockets natively, so
 * this costs nothing.
 *
 * The connection reconnects on its own, because `herdr-deckd` restarting (an upgrade, a crash,
 * a `service restart`) must not require the user to touch the Stream Deck app.
 */

import { createConnection, type Socket } from "node:net";
import { EventEmitter } from "node:events";
import { homedir } from "node:os";
import { join } from "node:path";

import {
	encode,
	FRONTEND_PROTOCOL,
	LineDecoder,
	type DaemonMessage,
	type DeviceReport,
	type FrontendMessage,
} from "./protocol.js";

const RECONNECT_MIN_MS = 500;
const RECONNECT_MAX_MS = 10_000;

/**
 * Where the daemon listens. Mirrors `herdr-deckd`'s own resolution.
 *
 * macOS has no `XDG_RUNTIME_DIR`, so the fallback is the path the daemon uses there.
 */
export function defaultSocketPath(): string {
	const explicit = process.env["HERDR_DECK_SOCKET"];
	if (explicit) {
		return explicit;
	}
	const runtime = process.env["XDG_RUNTIME_DIR"];
	if (runtime) {
		return join(runtime, "herdr-deck.sock");
	}
	return join(homedir(), "Library", "Application Support", "herdr-deck", "herdr-deck.sock");
}

export interface DaemonClientEvents {
	message: (message: DaemonMessage) => void;
	connected: () => void;
	disconnected: (reason: string) => void;
}

/**
 * Connects to the daemon, re-announcing the device on every reconnect.
 *
 * Re-sending `hello` matters: a restarted daemon has no memory of us, and without it the deck
 * would sit dark until the user unplugged something.
 */
export class DaemonClient extends EventEmitter {
	private socket: Socket | null = null;
	private decoder = new LineDecoder();
	private reconnectDelay = RECONNECT_MIN_MS;
	private reconnectTimer: NodeJS.Timeout | null = null;
	private closed = false;
	private device: DeviceReport | null = null;

	constructor(private readonly socketPath: string = defaultSocketPath()) {
		super();
	}

	/** Report the attached hardware. Sent immediately, and again after every reconnect. */
	setDevice(device: DeviceReport): void {
		this.device = device;
		if (this.socket && !this.socket.destroyed) {
			this.sendHello();
		}
	}

	connect(): void {
		if (this.closed || this.socket) {
			return;
		}
		const socket = createConnection(this.socketPath);
		this.socket = socket;
		socket.setEncoding("utf8");

		socket.on("connect", () => {
			this.reconnectDelay = RECONNECT_MIN_MS;
			this.decoder.reset();
			this.sendHello();
			this.emit("connected");
		});

		socket.on("data", (chunk: string) => {
			for (const message of this.decoder.push(chunk)) {
				this.emit("message", message);
			}
		});

		// `error` and `close` both fire on a failed connection; funnel them through one path so
		// we never schedule two reconnect timers for the same drop.
		socket.on("error", (error: Error) => this.drop(error.message));
		socket.on("close", () => this.drop("connection closed"));
	}

	private drop(reason: string): void {
		if (!this.socket) {
			return;
		}
		this.socket.removeAllListeners();
		this.socket.destroy();
		this.socket = null;
		this.emit("disconnected", reason);
		this.scheduleReconnect();
	}

	private scheduleReconnect(): void {
		if (this.closed || this.reconnectTimer) {
			return;
		}
		this.reconnectTimer = setTimeout(() => {
			this.reconnectTimer = null;
			this.connect();
		}, this.reconnectDelay);
		// Do not hold the process open just to retry.
		this.reconnectTimer.unref?.();
		this.reconnectDelay = Math.min(this.reconnectDelay * 2, RECONNECT_MAX_MS);
	}

	private sendHello(): void {
		if (!this.device) {
			return;
		}
		this.send({
			type: "hello",
			frontend: "streamdeck-macos",
			device: this.device,
			protocol: FRONTEND_PROTOCOL,
		});
	}

	send(message: FrontendMessage): void {
		if (!this.socket || this.socket.destroyed) {
			return;
		}
		this.socket.write(encode(message));
	}

	close(): void {
		this.closed = true;
		if (this.reconnectTimer) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
		this.socket?.destroy();
		this.socket = null;
	}

	get connected(): boolean {
		return this.socket !== null && !this.socket.destroyed;
	}
}
