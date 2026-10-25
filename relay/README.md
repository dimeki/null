# Listening-party relay

A tiny WebSocket server that connects the people in a listening party. It only
forwards small JSON messages (what is playing, where, paused or not). It never
carries audio and keeps nothing on disk: a room exists only while its host is
connected.

**It must run as exactly one instance.** Rooms live in memory, so if a host runs
two copies, two people in the same party can land on different ones and the
second can't find the room. `check.ts` (below) tests for this.

## Try it on your own PC

```
vendor\deno.exe run --allow-net --allow-env relay\relay.ts
```

Port 8787 (set `PORT` to change it). In the app, open Settings and set
**Party server** to `ws://127.0.0.1:8787`.

## Put it online for everyone

Pick one place that can run a small always-on program. `relay.ts` needs nothing
but Deno; the `Dockerfile` here is the easy way on hosts that take containers.

| Where | Cost | Notes |
|---|---|---|
| **Small VPS** (any provider) | a few dollars a month | The most predictable option. Run the container (or `deno run`) and put a TLS proxy such as Caddy in front. |
| **Fly.io** | a few dollars a month | `fly.toml` is included. Run exactly one machine and leave auto-stop off. I couldn't verify Fly's current free allowance. |
| **Render, free web service** | free | Sleeps after about 15 idle minutes, so the first party after a quiet spell waits for it to wake. |
| **A spare always-on PC or Raspberry Pi** | free | Run the relay there and expose it with a Cloudflare Tunnel, which gives you a `wss://` address without opening ports. |
| **Deno Deploy** | free tier | I could not confirm whether it keeps all connections on one instance. Run `check.ts` before relying on it. |

The Dockerfile and `fly.toml` are untested (no Docker or Fly tools on the machine
they were written on). The relay itself and `check.ts` are tested.

Whatever you pick, the address must start with `wss://` (TLS) once it's online.

## Run it on a home server

Two parts: run the relay, then give the internet a safe way to reach it. None of
the steps below were tried on a real home server; the relay itself is tested.

**1. Run the relay** (pick one)

- **Docker:** `docker build -t ytmusic-relay .` then
  `docker run -d --restart unless-stopped -p 127.0.0.1:8787:8787 --name ytmusic-relay ytmusic-relay`
- **Linux without Docker:** install Deno, copy `relay.ts` to `/opt/ytmusic-relay/`, and use the
  included `ytmusic-relay.service` (it restarts the relay if it stops and after reboots).
- **Windows:** `vendor\deno.exe run --allow-net --allow-env relay\relay.ts`, started at logon
  with Task Scheduler.

Set `HOST=127.0.0.1` when a tunnel or proxy runs on the same machine, so only it can reach
the relay.

**2. Reach it from outside** (pick one; the app needs a `wss://` address)

| Way | Router changes | Stable address | Notes |
|---|---|---|---|
| **Cloudflare Tunnel** | none | yes, on a domain you own and keep on Cloudflare | `cloudflared` makes an outbound connection to Cloudflare. A named tunnel needs a domain. `cloudflared tunnel --url http://localhost:8787` gives a free temporary address that changes every run, fine for testing only. |
| **Tailscale Funnel** | none | yes, a `*.ts.net` name | `tailscale funnel --bg 8787` after enabling Funnel for your tailnet. Check Tailscale's current Funnel limits. |
| **Port forwarding + Caddy** | forward port 443 | yes, with a dynamic-DNS name | Caddy fetches a TLS certificate and forwards to the relay. Needs a public IP address (not CGNAT), and opens a port on your home network. |

The relay limits each visitor by the address the tunnel reports, so a busy party isn't
throttled as one visitor. Cloudflare and most proxies pass the real address along. If yours
doesn't, raise `MAX_CONNECTIONS_PER_IP` and `MAX_CREATES_PER_MINUTE` (environment variables).

**Keep in mind:** the party only works while the home server and its internet connection are up.

## Check it works

```
vendor\deno.exe run --allow-net relay\check.ts wss://your-relay.example.com
```

It creates and joins several parties in a row. `8 of 8 parties worked` means
good. If some guests "couldn't find" a party that was just created, the host is
running more than one instance.

## Ship the address in the app

Put the address in `src\config.rs` (`DEFAULT_PARTY_SERVER`) and rebuild. From
then on nobody has to enter a server: the Party tab just works, and Settings keeps
an optional override for anyone who wants their own relay.

## Tests

```
cd relay
..\vendor\deno.exe test --allow-net relay_test.ts
```

## Limits and privacy

- Up to 500 rooms and 25 guests per room; messages up to 64 KB.
- Per network address: 20 simultaneous connections and 10 new parties a minute.
- A connection that sends nothing for two minutes is closed (the app pings every 15 seconds).
- `GET /healthz` answers `ok` for health checks.
- Anyone who has a room code can join that room, and the relay can see song
  titles and display names, so share codes only with people you trust.
