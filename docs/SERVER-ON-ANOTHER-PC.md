# Your server on another PC

One player, your own saves, your server running on a second PC on your home
network: a desktop in the garage that stays on, a laptop you play from. The
game always runs on the PC you play on; the server owns the saves, the world
settings and the world. This page is the setup, first with the desktop
launcher and then with the terminal `Afterhours.exe`.

Everything here is one player on one home network. Several players, access
by account and the open internet are not supported: the server speaks plain
HTTP with a token, and anyone on the network who learns the token can
control it.

## What runs where

| On the server PC                          | On the PC you play on                              |
| ----------------------------------------- | -------------------------------------------------- |
| `Afterhours.exe serve --server-only`      | the game, started by the launcher                  |
| the saves (`artifacts/account/*.sqlite`)  | nothing of the save                                |
| world settings (traffic, rivals)          | car pictures and the map, decoded from its install |
| the session API (status, log, settings)   | a small relay the game talks to on `127.0.0.1`     |

The game's backend address is a 10-byte field that holds `127.0.0.1` but
not a typical network address, so the game is never given the server's
address: the launcher starts a relay on the playing PC under the name the
game looks for, and the lease routes the game to it. The relay forwards the
redirector connection and launcher services unchanged; the server's own
replies give the game its Blaze, QoS and world addresses, which are on the
server PC. Live checks on 2026-10-21 played this way against a server on
the same PC's network address (afterhours-2015#172).

Both PCs need the supported game installed: the server PC reads installed
content once when it prepares, the playing PC runs the game from its copy.

## With the desktop launcher

On the **server PC**:

1. Open the launcher, go to **Server**, pick **This PC** and turn on **Let
   your other PCs use this server**. It takes effect when the server next
   starts.
2. Press **Serve**. The server now listens on this PC's network address
   (shown in the chip), without game support on this PC. Windows may ask
   to allow `Afterhours.exe` through the firewall: allow it on private
   networks.
3. Press **Copy** beside the pairing code. The code is
   `<address>:<port>/<token>`; the token stays the same across restarts.

On the **PC you play on**:

4. Go to **Server**, press **Add a server**, give it a name, paste the
   pairing code and press **Check**. The launcher asks that server what it
   is and shows its version, what works here and how many drivers are
   saved there. If it says *not reachable*, see below.
5. Press **Save and use**. Play, World, Profile, Map and Logs now follow
   that server. Press **Play**: the game starts here, relayed to the
   server. Your REP and cars are saved on the server PC.

Pick **This PC** again on Server to go back to your own server. Picking
another server stops this PC's own unless it is shared. Closing the
launcher while this PC's shared server runs asks whether to keep it running
for your other PCs; Settings remembers the answer.

## With the terminal

On the server PC, in a terminal:

```text
Afterhours.exe serve --server-only --api --listen <this PC's IPv4 address> --api-port <port>
Afterhours.exe api --json
```

`api --json` prints the address and token; the pairing code for a launcher
is `<address>:<port>/<token>`. `--listen` binds the server and its API to
that one address of this PC (`ipconfig` shows it), `--api-port` keeps the
API on a fixed port so the other PC finds it after a restart, and the token
is kept in `api-token` in the data directory. The session runs until
`stop`, Ctrl+C, or a launcher's Stop.

On the playing PC, without the desktop launcher:

```text
Afterhours.exe start --remote http://<address>:<port> --token <token>
```

This runs the usual game checks here, starts the relay and the game, and
reports the session as `start` does. The crate README's
[Session API](../crates/launcher/README.md#session-api) section has every
route and option.

## When it does not connect

- **Not reachable.** Is the server PC on, and is the session serving (its
  Server screen or terminal says *ready*)? Both PCs must be on the same
  network, and the server must have been started with sharing on
  (`--listen`), not on `127.0.0.1`. Windows Firewall on the server PC may
  have been declined: allow `Afterhours.exe` on private networks, or open
  the API port and the game ports its startup report lists.
- **Token refused.** The token changed (the `api-token` file was deleted or
  the server moved to another data directory). Copy the pairing code again
  and use *New pairing code* on the saved server.
- **Older server.** The server has no session API (before 0.4.1). Update it;
  the launcher bundles the version it needs.
- **The game does not start here.** The playing PC's own checks apply: the
  supported build, the activation helper, no EA app running, port 3216 free.
  Its session log under Logs says which one refused.

Stop the server from any PC's launcher that follows it (Server › Stop
server), or with `stop` on the server PC.
