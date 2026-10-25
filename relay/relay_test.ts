// Run with:  deno test --allow-net relay_test.ts
import { roomCount, startRelay } from "./relay.ts";

function assert(cond: unknown, message: string): asserts cond {
  if (!cond) throw new Error(`assertion failed: ${message}`);
}

function assertEquals<T>(actual: T, expected: T, message = "") {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`${message} expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

/** A tiny test client that remembers every message it receives */
class Client {
  socket: WebSocket;
  inbox: Record<string, unknown>[] = [];
  private waiters: Array<() => void> = [];

  private constructor(socket: WebSocket) {
    this.socket = socket;
    socket.onmessage = (e) => {
      this.inbox.push(JSON.parse(e.data));
      this.waiters.splice(0).forEach((w) => w());
    };
  }

  static async connect(port: number): Promise<Client> {
    const socket = new WebSocket(`ws://127.0.0.1:${port}`);
    await new Promise<void>((resolve, reject) => {
      socket.onopen = () => resolve();
      socket.onerror = () => reject(new Error("could not connect"));
    });
    return new Client(socket);
  }

  send(message: unknown) {
    this.socket.send(JSON.stringify(message));
  }

  /** Waits for the next message of this type that has not been consumed yet */
  async next(type: string, timeoutMs = 2000): Promise<Record<string, unknown>> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const i = this.inbox.findIndex((m) => m.t === type);
      if (i >= 0) return this.inbox.splice(i, 1)[0];
      const left = deadline - Date.now();
      if (left <= 0) throw new Error(`timed out waiting for "${type}" (inbox: ${JSON.stringify(this.inbox)})`);
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, left);
        this.waiters.push(() => {
          clearTimeout(timer);
          resolve();
        });
      });
    }
  }

  async expectNothing(type: string, forMs = 300) {
    await new Promise((r) => setTimeout(r, forMs));
    assert(!this.inbox.some((m) => m.t === type), `unexpected "${type}" message: ${JSON.stringify(this.inbox)}`);
  }

  close() {
    this.socket.close();
  }
}

async function withRelay(test: (port: number) => Promise<void>, options: Parameters<typeof startRelay>[2] = {}) {
  // generous limits by default so the other tests are not throttled by them
  const server = startRelay(0, "127.0.0.1", { maxCreatesPerMinute: 1000, maxConnectionsPerIp: 1000, ...options });
  try {
    await test((server.addr as Deno.NetAddr).port);
  } finally {
    await server.shutdown();
  }
}

Deno.test({
  name: "ping returns the relay's clock",
  sanitizeResources: false,
  sanitizeOps: false,
  fn: () =>
    withRelay(async (port) => {
      const c = await Client.connect(port);
      c.send({ t: "ping", c: 12345 });
      const pong = await c.next("pong");
      assertEquals(pong.c, 12345);
      assert(typeof pong.s === "number" && Math.abs((pong.s as number) - Date.now()) < 5000, "server time");
      c.close();
    }),
});

Deno.test({
  name: "creating a party gives a readable six-character code",
  sanitizeResources: false,
  sanitizeOps: false,
  fn: () =>
    withRelay(async (port) => {
      const host = await Client.connect(port);
      host.send({ t: "create", name: "  Giovanni  " });
      const created = await host.next("created");
      assert(/^[A-HJKMNP-Z2-9]{6}$/.test(created.room as string), `code ${created.room}`);
      const members = await host.next("members");
      assertEquals((members.members as Array<{ name: string; host: boolean }>).map((m) => [m.name, m.host]), [["Giovanni", true]]);
      host.close();
    }),
});

Deno.test({
  name: "joining: unknown codes are refused, real ones work and the host is told",
  sanitizeResources: false,
  sanitizeOps: false,
  fn: () =>
    withRelay(async (port) => {
      const host = await Client.connect(port);
      host.send({ t: "create", name: "Host" });
      const { room } = await host.next("created") as { room: string };
      await host.next("members");

      const guest = await Client.connect(port);
      guest.send({ t: "join", room: "ZZZZZZ", name: "Nope" });
      assertEquals((await guest.next("error")).message, "That party code wasn't found");

      guest.send({ t: "join", room: room.toLowerCase(), name: "Guest" }); // codes are case-insensitive
      const joined = await guest.next("joined");
      assertEquals(joined.host, "Host");
      await host.next("hello");
      const hostView = await host.next("members");
      assertEquals((hostView.members as Array<{ name: string }>).map((m) => m.name), ["Host", "Guest"]);
      host.close();
      guest.close();
    }),
});

Deno.test({
  name: "only the host's state reaches guests, and late joiners get the latest one",
  sanitizeResources: false,
  sanitizeOps: false,
  fn: () =>
    withRelay(async (port) => {
      const host = await Client.connect(port);
      host.send({ t: "create", name: "Host" });
      const { room } = await host.next("created") as { room: string };

      const early = await Client.connect(port);
      early.send({ t: "join", room, name: "Early" });
      await early.next("joined");

      host.send({ t: "state", track: { id: "abc" }, playing: true, pos: 12.5, ts: 1 });
      const got = await early.next("state");
      assertEquals(got.pos, 12.5);
      assertEquals((got.track as { id: string }).id, "abc");

      // a guest trying to steer playback is ignored
      early.send({ t: "state", track: { id: "evil" }, playing: true, pos: 0, ts: 2 });
      const other = await Client.connect(port);
      other.send({ t: "join", room, name: "Late" });
      await other.next("joined");
      const replay = await other.next("state");
      assertEquals((replay.track as { id: string }).id, "abc", "late joiner sees the host's state");
      await early.expectNothing("state");

      host.close();
      early.close();
      other.close();
    }),
});

Deno.test({
  name: "when the host leaves every guest is told and the code stops working",
  sanitizeResources: false,
  sanitizeOps: false,
  fn: () =>
    withRelay(async (port) => {
      const host = await Client.connect(port);
      host.send({ t: "create", name: "Host" });
      const { room } = await host.next("created") as { room: string };
      const guest = await Client.connect(port);
      guest.send({ t: "join", room, name: "Guest" });
      await guest.next("joined");
      assertEquals(roomCount(), 1);

      host.send({ t: "leave" });
      const closed = await guest.next("closed");
      assert(String(closed.reason).includes("host"), "reason mentions the host");
      assertEquals(roomCount(), 0);

      const late = await Client.connect(port);
      late.send({ t: "join", room, name: "Late" });
      assertEquals((await late.next("error")).message, "That party code wasn't found");
      late.close();
      guest.close();
      host.close();
    }),
});

Deno.test({
  name: "one network address cannot start parties endlessly",
  sanitizeResources: false,
  sanitizeOps: false,
  fn: () =>
    withRelay(async (port) => {
      const clients: Client[] = [];
      for (let i = 0; i < 3; i++) {
        const c = await Client.connect(port);
        clients.push(c);
        c.send({ t: "create", name: `Host ${i}` });
        await c.next("created");
      }
      const fourth = await Client.connect(port);
      fourth.send({ t: "create", name: "One too many" });
      const refused = await fourth.next("error");
      assert(String(refused.message).includes("Too many parties"), "a clear message");
      assertEquals(roomCount(), 3, "no fourth room was made");
      [...clients, fourth].forEach((c) => c.close());
    }, { maxCreatesPerMinute: 3 }),
});

Deno.test({
  name: "too many simultaneous connections from one address are refused",
  sanitizeResources: false,
  sanitizeOps: false,
  fn: () =>
    withRelay(async (port) => {
      const a = await Client.connect(port);
      const b = await Client.connect(port);
      const refused = await fetch(`http://127.0.0.1:${port}/`, { headers: { Upgrade: "websocket", Connection: "Upgrade", "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==", "Sec-WebSocket-Version": "13" } });
      assertEquals(refused.status, 429);
      await refused.body?.cancel();
      a.close();
      b.close();
    }, { maxConnectionsPerIp: 2 }),
});

Deno.test({
  name: "behind a proxy, limits count each visitor by the address the proxy reports",
  sanitizeResources: false,
  sanitizeOps: false,
  fn: () =>
    withRelay(async (port) => {
      const asVisitor = (ip: string) =>
        new Promise<WebSocket | null>((resolve) => {
          // Deno lets a WebSocket client set request headers, standing in for the proxy here
          const ws = new WebSocket(`ws://127.0.0.1:${port}`, { headers: { "cf-connecting-ip": ip } } as unknown as string[]);
          ws.onopen = () => resolve(ws);
          ws.onerror = () => resolve(null);
        });
      const first = await asVisitor("203.0.113.1");
      const second = await asVisitor("203.0.113.2");
      assert(first && second, "two different visitors each get their own allowance");
      const again = await asVisitor("203.0.113.1");
      assert(again === null, "the first visitor is over their own limit");
      first.close();
      second.close();
    }, { maxConnectionsPerIp: 1 }),
});

Deno.test({
  name: "a connection that goes quiet is closed, while one that keeps pinging stays",
  sanitizeResources: false,
  sanitizeOps: false,
  fn: () =>
    withRelay(async (port) => {
      const quiet = await Client.connect(port);
      const lively = await Client.connect(port);
      const closed = new Promise<boolean>((resolve) => {
        quiet.socket.onclose = () => resolve(true);
        setTimeout(() => resolve(false), 1500);
      });
      const keepAlive = setInterval(() => lively.send({ t: "ping", c: 1 }), 150);
      assert(await closed, "the quiet connection was closed");
      clearInterval(keepAlive);
      assertEquals(lively.socket.readyState, WebSocket.OPEN);
      lively.close();
    }, { idleTimeoutMs: 400 }),
});

Deno.test({
  name: "the update file is served from disk, and problems are reported plainly",
  sanitizeResources: false,
  sanitizeOps: false,
  fn: async () => {
    const dir = await Deno.makeTempDir();
    const file = `${dir}/latest.json`;
    const manifest = JSON.stringify({ version: "1.2.0", url: "https://example.com/get", notes: "n" });
    await withRelay(async (port) => {
      const get = () => fetch(`http://127.0.0.1:${port}/latest.json`);
      const missing = await get();
      assertEquals(missing.status, 404);
      await missing.body?.cancel();

      await Deno.writeTextFile(file, manifest);
      const ok = await get();
      assertEquals(ok.status, 200);
      assertEquals(await ok.text(), manifest);

      // edited while the relay is running: the next request sees it
      await Deno.writeTextFile(file, JSON.stringify({ version: "1.3.0", url: "https://example.com/get2" }));
      assertEquals(((await (await get()).json()) as { version: string }).version, "1.3.0");

      await Deno.writeTextFile(file, "{ not json");
      const broken = await get();
      assertEquals(broken.status, 500);
      await broken.body?.cancel();
    }, { manifestPath: file });
    await Deno.remove(dir, { recursive: true });
  },
});

Deno.test({
  name: "the health check answers",
  sanitizeResources: false,
  sanitizeOps: false,
  fn: () =>
    withRelay(async (port) => {
      const res = await fetch(`http://127.0.0.1:${port}/healthz`);
      assertEquals(await res.text(), "ok\n");
    }),
});

Deno.test({
  name: "a guest's sync status reaches the host; a host cannot set one",
  sanitizeResources: false,
  sanitizeOps: false,
  fn: () =>
    withRelay(async (port) => {
      const host = await Client.connect(port);
      host.send({ t: "create", name: "Host" });
      const { room } = await host.next("created") as { room: string };
      const guest = await Client.connect(port);
      guest.send({ t: "join", room, name: "Guest" });
      await guest.next("joined");

      guest.send({ t: "status", sync: "Catching up…" });
      let seen = "";
      for (let i = 0; i < 6 && seen !== "Catching up…"; i++) {
        const m = await host.next("members");
        seen = (m.members as Array<{ name: string; sync: string }>).find((x) => x.name === "Guest")?.sync ?? "";
      }
      assertEquals(seen, "Catching up…");

      host.send({ t: "status", sync: "hacked" });
      await new Promise((r) => setTimeout(r, 200));
      host.inbox.length = 0;
      guest.send({ t: "status", sync: "x".repeat(100) });
      const after = await host.next("members");
      const guestEntry = (after.members as Array<{ name: string; sync: string; host: boolean }>);
      assertEquals(guestEntry.find((x) => x.host)?.sync, "", "the host's own entry stays empty");
      assertEquals(guestEntry.find((x) => x.name === "Guest")?.sync.length, 24, "long reports are cut short");
      host.close();
      guest.close();
    }),
});

Deno.test({
  name: "a guest leaving updates the member list; junk input is ignored",
  sanitizeResources: false,
  sanitizeOps: false,
  fn: () =>
    withRelay(async (port) => {
      const host = await Client.connect(port);
      host.send({ t: "create", name: "Host" });
      const { room } = await host.next("created") as { room: string };
      await host.next("members");
      const guest = await Client.connect(port);
      guest.send({ t: "join", room, name: "Guest" });
      await guest.next("joined");
      await host.next("members"); // Host + Guest

      host.socket.send("not json at all");
      host.socket.send("[1,2,3]");
      host.send({ t: "something-unknown" });

      guest.send({ t: "leave" });
      const after = await host.next("members");
      assertEquals((after.members as Array<{ name: string }>).map((m) => m.name), ["Host"]);
      assertEquals(roomCount(), 1, "the room survives a guest leaving");
      host.close();
      guest.close();
    }),
});
