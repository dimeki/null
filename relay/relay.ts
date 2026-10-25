// Listening-party relay for ytmusic-lite.
//
// It only forwards small JSON messages (what is playing, where, paused or not)
// between the people in a room. It never sees or carries audio, and it keeps
// nothing on disk: a room exists only while its host is connected.
//
// Run locally:  deno run --allow-net --allow-env relay.ts        (PORT env, default 8787)
// Deploy:       the same file runs unchanged on Deno Deploy (https://deno.com/deploy)
//
// Protocol (every message is a JSON object with a "t" field):
//   client -> relay   ping {c}                 relay -> client  pong {c, s}   (s = relay clock, ms)
//                     create {name}                             created {room, you}
//                     join {room, name}                         joined {room, you, host}
//                     state {...}  (host only)                  state {...}    (to every guest)
//                     status {sync}  (guest only)               members {members:[{id,name,host,sync}]}
//                     leave
//                                                               hello         (to the host: a guest joined, resend state)
//                                                               closed {reason}
//                                                               error {message}

const ALPHABET = "ABCDEFGHJKMNPQRSTUVWXYZ23456789"; // no 0/O, 1/I/L, so codes can be read aloud
const CODE_LENGTH = 6;
const MAX_ROOMS = 500;
const MAX_GUESTS = 25;
const MAX_MESSAGE_CHARS = 64 * 1024;
const MAX_STATES_PER_SECOND = 8;
const MAX_NAME_CHARS = 24;

/** Limits that protect a public relay. Tests tighten or loosen them through startRelay. */
interface Limits {
  maxConnectionsPerIp: number;
  maxCreatesPerMinute: number;
  /** A connection that sends nothing for this long is closed (the app pings every 15 s) */
  idleTimeoutMs: number;
}

const DEFAULT_LIMITS: Limits = { maxConnectionsPerIp: 20, maxCreatesPerMinute: 10, idleTimeoutMs: 120_000 };
let limits: Limits = { ...DEFAULT_LIMITS };
const connectionsByIp = new Map<string, number>();
const createsByIp = new Map<string, number[]>();

function allowCreate(ip: string): boolean {
  const now = Date.now();
  if (createsByIp.size > 5000) {
    for (const [key, times] of createsByIp) {
      if (times.every((t) => now - t >= 60_000)) createsByIp.delete(key);
    }
  }
  const recent = (createsByIp.get(ip) ?? []).filter((t) => now - t < 60_000);
  const allowed = recent.length < limits.maxCreatesPerMinute;
  if (allowed) recent.push(now);
  createsByIp.set(ip, recent);
  return allowed;
}

interface Member {
  id: string;
  name: string;
  socket: WebSocket;
  isHost: boolean;
  /** A guest's own report of how well it is keeping up, shown to the host */
  sync: string;
}

interface Room {
  code: string;
  host: Member;
  guests: Map<string, Member>;
  /** The host's latest "state" message, replayed to people who join late */
  lastState: string | null;
}

const rooms = new Map<string, Room>();

function send(socket: WebSocket, message: unknown) {
  if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
}

function everyone(room: Room): Member[] {
  return [room.host, ...room.guests.values()];
}

function broadcastMembers(room: Room) {
  const members = everyone(room).map((m) => ({ id: m.id, name: m.name, host: m.isHost, sync: m.sync }));
  for (const m of everyone(room)) send(m.socket, { t: "members", members });
}

function newCode(): string {
  for (;;) {
    const bytes = crypto.getRandomValues(new Uint8Array(CODE_LENGTH));
    const code = Array.from(bytes, (b) => ALPHABET[b % ALPHABET.length]).join("");
    if (!rooms.has(code)) return code;
  }
}

function cleanName(raw: unknown): string {
  const name = typeof raw === "string" ? raw.replace(/[\u0000-\u001f]/g, "").trim() : "";
  return (name || "Guest").slice(0, MAX_NAME_CHARS);
}

function handle(socket: WebSocket, ip: string) {
  let me: Member | null = null;
  let room: Room | null = null;
  let windowStart = 0;
  let statesInWindow = 0;

  // drop connections that have gone quiet (a dead client never says goodbye)
  let idleTimer: ReturnType<typeof setTimeout> | undefined;
  const touch = () => {
    clearTimeout(idleTimer);
    idleTimer = setTimeout(() => {
      try {
        socket.close(1001, "idle");
      } catch { /* already closed */ }
    }, limits.idleTimeoutMs);
  };
  touch();

  const leave = (reason: string) => {
    if (!me || !room) return;
    if (me.isHost) {
      rooms.delete(room.code);
      for (const g of room.guests.values()) {
        send(g.socket, { t: "closed", reason });
        try {
          g.socket.close(1000, "party ended");
        } catch { /* already closed */ }
      }
    } else {
      room.guests.delete(me.id);
      broadcastMembers(room);
    }
    me = null;
    room = null;
  };

  socket.onmessage = (event) => {
    touch();
    if (typeof event.data !== "string" || event.data.length > MAX_MESSAGE_CHARS) {
      socket.close(1009, "message too large");
      return;
    }
    let msg: Record<string, unknown>;
    try {
      const parsed = JSON.parse(event.data);
      if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return;
      msg = parsed;
    } catch {
      return;
    }

    switch (msg.t) {
      case "ping":
        send(socket, { t: "pong", c: msg.c, s: Date.now() });
        break;

      case "create": {
        if (me) return send(socket, { t: "error", message: "You're already in a party" });
        if (rooms.size >= MAX_ROOMS) return send(socket, { t: "error", message: "The server is full right now" });
        if (!allowCreate(ip)) {
          return send(socket, { t: "error", message: "Too many parties started from your network. Try again in a minute." });
        }
        me = { id: crypto.randomUUID(), name: cleanName(msg.name), socket, isHost: true, sync: "" };
        room = { code: newCode(), host: me, guests: new Map(), lastState: null };
        rooms.set(room.code, room);
        send(socket, { t: "created", room: room.code, you: me.id });
        broadcastMembers(room);
        break;
      }

      case "join": {
        if (me) return send(socket, { t: "error", message: "You're already in a party" });
        const code = typeof msg.room === "string" ? msg.room.trim().toUpperCase() : "";
        const target = rooms.get(code);
        if (!target) return send(socket, { t: "error", message: "That party code wasn't found" });
        if (target.guests.size >= MAX_GUESTS) return send(socket, { t: "error", message: "That party is full" });
        me = { id: crypto.randomUUID(), name: cleanName(msg.name), socket, isHost: false, sync: "" };
        room = target;
        room.guests.set(me.id, me);
        send(socket, { t: "joined", room: room.code, you: me.id, host: room.host.name });
        if (room.lastState) socket.send(room.lastState);
        broadcastMembers(room);
        // the stored state may be stale by now, so ask the host for a fresh one
        send(room.host.socket, { t: "hello" });
        break;
      }

      case "state": {
        if (!me?.isHost || !room) return; // only the host steers playback
        const now = Date.now();
        if (now - windowStart > 1000) {
          windowStart = now;
          statesInWindow = 0;
        }
        if (++statesInWindow > MAX_STATES_PER_SECOND) return;
        const forwarded = JSON.stringify(msg);
        room.lastState = forwarded;
        for (const g of room.guests.values()) {
          if (g.socket.readyState === WebSocket.OPEN) g.socket.send(forwarded);
        }
        break;
      }

      case "status": {
        // a guest tells the host how it is doing ("In sync", "Catching up…")
        if (!me || me.isHost || !room) return;
        const sync = typeof msg.sync === "string" ? msg.sync.replace(/[\u0000-\u001f]/g, "").trim().slice(0, 24) : "";
        if (sync === me.sync) return;
        me.sync = sync;
        broadcastMembers(room);
        break;
      }

      case "leave":
        leave("The host ended the party");
        break;
    }
  };

  socket.onclose = () => {
    clearTimeout(idleTimer);
    leave("The host's connection dropped");
  };
  socket.onerror = () => leave("The host's connection dropped");
}

/** The caller's address, looking through the proxy a host puts in front of the relay */
function clientIp(req: Request, remote: Deno.Addr): string {
  const forwarded = req.headers.get("cf-connecting-ip") ?? req.headers.get("fly-client-ip") ??
    req.headers.get("x-forwarded-for")?.split(",")[0].trim();
  return forwarded || (remote as Deno.NetAddr).hostname || "unknown";
}

/** Serves the "newest version" file the app checks for updates. It is read on every
 *  request, so publishing an update is just editing that file; no restart needed. */
async function serveUpdateManifest(path: string): Promise<Response> {
  const headers = { "content-type": "application/json", "cache-control": "max-age=300" };
  let text: string;
  try {
    text = await Deno.readTextFile(path);
  } catch {
    return new Response(JSON.stringify({ error: "no update information published" }), { status: 404, headers });
  }
  try {
    JSON.parse(text);
  } catch {
    return new Response(JSON.stringify({ error: "the update file is not valid JSON" }), { status: 500, headers });
  }
  return new Response(text, { headers });
}

export function startRelay(
  port: number,
  hostname = "0.0.0.0",
  options: Partial<Limits> & { manifestPath?: string } = {},
): Deno.HttpServer {
  const { manifestPath = "latest.json", ...limitOverrides } = options;
  limits = { ...DEFAULT_LIMITS, ...limitOverrides };
  connectionsByIp.clear();
  createsByIp.clear();
  return Deno.serve({ port, hostname, onListen: () => {} }, (req, info) => {
    const path = new URL(req.url).pathname;
    if (path === "/healthz") return new Response("ok\n");
    if (path === "/latest.json") return serveUpdateManifest(manifestPath);
    if (req.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      return new Response("ytmusic-lite listening-party relay\n");
    }
    const ip = clientIp(req, info.remoteAddr);
    const open = connectionsByIp.get(ip) ?? 0;
    if (open >= limits.maxConnectionsPerIp) return new Response("Too many connections\n", { status: 429 });
    connectionsByIp.set(ip, open + 1);
    const { socket, response } = Deno.upgradeWebSocket(req);
    socket.addEventListener("close", () => {
      const left = (connectionsByIp.get(ip) ?? 1) - 1;
      if (left <= 0) connectionsByIp.delete(ip);
      else connectionsByIp.set(ip, left);
    });
    handle(socket, ip);
    return response;
  });
}

export function roomCount(): number {
  return rooms.size;
}

if (import.meta.main) {
  // Settings can be changed with environment variables:
  //   PORT (8787), HOST (0.0.0.0; use 127.0.0.1 when a proxy or tunnel on the same machine fronts it),
  //   MAX_CONNECTIONS_PER_IP (20), MAX_CREATES_PER_MINUTE (10),
  //   MANIFEST_PATH (latest.json: the file served at /latest.json for app update checks)
  const number = (name: string, fallback: number) => {
    const value = Number(Deno.env.get(name));
    return Number.isFinite(value) && value > 0 ? value : fallback;
  };
  const port = number("PORT", 8787);
  const host = Deno.env.get("HOST") ?? "0.0.0.0";
  startRelay(port, host, {
    manifestPath: Deno.env.get("MANIFEST_PATH") ?? "latest.json",
    maxConnectionsPerIp: number("MAX_CONNECTIONS_PER_IP", DEFAULT_LIMITS.maxConnectionsPerIp),
    maxCreatesPerMinute: number("MAX_CREATES_PER_MINUTE", DEFAULT_LIMITS.maxCreatesPerMinute),
  });
  console.log(`Listening-party relay running on ${host}:${port}`);
}
