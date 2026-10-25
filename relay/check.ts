// Checks that a running relay really works for parties: it starts rooms and joins
// them from separate connections, several times over. If a host runs the relay as
// more than one instance, some rooms can't be found by the second connection and
// this reports it.
//
// Usage:  deno run --allow-net check.ts wss://your-relay.example.com [rounds]
//
// It stays under the relay's limit of 10 new parties per minute per network
// address, so keep rounds at 8 or fewer.

const url = Deno.args[0];
const rounds = Math.min(Number(Deno.args[1] ?? 8), 8);
if (!url || !/^wss?:\/\//.test(url)) {
  console.error("Usage: deno run --allow-net check.ts wss://your-relay.example.com [rounds]");
  Deno.exit(2);
}

type Msg = Record<string, unknown>;

class Conn {
  inbox: Msg[] = [];
  private constructor(readonly ws: WebSocket) {
    ws.onmessage = (e) => this.inbox.push(JSON.parse(e.data));
  }

  static open(): Promise<Conn> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      const timer = setTimeout(() => reject(new Error("could not connect within 10 seconds")), 10_000);
      ws.onopen = () => {
        clearTimeout(timer);
        resolve(new Conn(ws));
      };
      ws.onerror = () => {
        clearTimeout(timer);
        reject(new Error("could not connect"));
      };
    });
  }

  send(m: Msg) {
    this.ws.send(JSON.stringify(m));
  }

  async wait(type: string, ms = 5000): Promise<Msg> {
    const deadline = Date.now() + ms;
    while (Date.now() < deadline) {
      const i = this.inbox.findIndex((m) => m.t === type || m.t === "error");
      if (i >= 0) {
        const m = this.inbox.splice(i, 1)[0];
        if (m.t === "error") throw new Error(String(m.message));
        return m;
      }
      await new Promise((r) => setTimeout(r, 25));
    }
    throw new Error(`no "${type}" message within ${ms / 1000} seconds`);
  }

  close() {
    try {
      this.ws.close();
    } catch { /* already closed */ }
  }
}

async function oneRound(): Promise<{ ms: number }> {
  const started = performance.now();
  const host = await Conn.open();
  const guest = await Conn.open();
  try {
    host.send({ t: "create", name: "Check host" });
    const { room } = await host.wait("created") as { room: string };
    guest.send({ t: "join", room, name: "Check guest" });
    await guest.wait("joined");
    host.send({ t: "state", track: null, playing: false, pos: 0, ts: Date.now() });
    await guest.wait("state");
    return { ms: performance.now() - started };
  } finally {
    host.close();
    guest.close();
  }
}

console.log(`Checking ${url} with ${rounds} parties...`);
let worked = 0;
const times: number[] = [];
const problems = new Map<string, number>();
for (let i = 1; i <= rounds; i++) {
  try {
    const { ms } = await oneRound();
    worked++;
    times.push(ms);
    console.log(`  round ${i}: ok (${Math.round(ms)} ms to create, join and sync)`);
  } catch (e) {
    const why = e instanceof Error ? e.message : String(e);
    problems.set(why, (problems.get(why) ?? 0) + 1);
    console.log(`  round ${i}: FAILED (${why})`);
  }
  await new Promise((r) => setTimeout(r, 400));
}

console.log(`\n${worked} of ${rounds} parties worked.`);
if (times.length) {
  times.sort((a, b) => a - b);
  console.log(`Typical time to create, join and sync: ${Math.round(times[Math.floor(times.length / 2)])} ms`);
}
if (worked === rounds) {
  console.log("Looks good: this host keeps each party on one instance.");
} else if ([...problems.keys()].some((p) => p.includes("wasn't found"))) {
  console.log(
    "Some guests could not find a party that had just been created. That usually means the host\n" +
      "is running the relay as several instances, so two connections land on different ones.\n" +
      "Run exactly one instance, or pick a host that does.",
  );
} else {
  console.log("Some rounds failed for the reasons shown above.");
}
Deno.exit(worked === rounds ? 0 : 1);
