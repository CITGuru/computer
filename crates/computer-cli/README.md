# computer-cli

The `computer` command.

```bash
computer up                        # opens a box, prints its id
computer open $BOX https://example.com
computer shot $BOX out.png
computer trace $BOX
computer rm $BOX
```

## It talks to a server, and starts one if it has to

Every command goes through a server. That is what lets the same ones reach a box
on this machine and a box in somebody's fleet:

```bash
export COMPUTER_SERVER_URL=https://boxes.example.com
export COMPUTER_SERVER_TOKEN=…
computer ls                        # their fleet, same command
```

When no server is named and none is listening on `127.0.0.1:8080`, one is
started here on a port the OS picks and dies with the command. So nothing has to
be running first.

That works because a box carries its own spec in a label and is taken back on
startup — a server that lives for one command is not amnesiac, it rediscovers
what is running each time. The probe checks *what* answered rather than that
something did, so an unrelated service on 8080 is not mistaken for a fleet.

## `--local`

Drives the box from this process, with no server and no socket:

```bash
computer --local shot $BOX out.png
```

Faster, and a smaller set. `fork` and `trace` are both built on a record of what
was done, and a process that exits when the command does has nowhere to keep
one. They say so rather than doing something lesser:

```
$ computer --local trace $BOX
trace needs a server to remember what was done, and --local has none.
Run it without --local.
```

The same limit applies without the flag whenever the server is the ephemeral
one: a trace only covers the command that is running. Run `computer-server` as a
daemon and it persists.

## Conventions

An id goes to standard output and everything a person reads goes to standard
error, so `computer shot $(computer up)` works.
