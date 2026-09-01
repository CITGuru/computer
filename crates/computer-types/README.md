# computer-types

The types a box is described and driven with.

Two halves. A `Spec` says what desktop is wanted:

```toml
[desktop]
server   = "x11"
features = ["wide_fonts"]

[policy]
network = true
auth    = "token"
```

And `Point`, `Button` and `Selection` are the values every caller names once it
has one. A server, a client, a CLI and eventually the engine all need both, and
none of them should be redefining a coordinate.

A spec says nothing about *where* it runs, which is what lets one travel
between a container, a microVM and somebody else's cloud. `Placement` carries
that half, and is kept out of the spec so two identical desktops that differ
only in a memory limit hash the same. `Spec::digest()` names a spec by its
contents, so the same desktop asked for in two key orders is one digest.

## Nothing here knows about an image

A spec that names no size is portable across images whose natural sizes differ,
and whatever compiles it applies its own defaults and its own limits — the
`computer-desktop` image runs eight screens and a macOS guest runs one, and
neither number belongs in the description.

So `width`, `height` and `screens` are all optional, and a spec that leaves them
open is a *different* spec from one that pins them, with a different digest. One
travels to an image that has its own idea of a screen; the other insists.

## Deliberately dependency-free

Serde and a digest. Everything that reads these depends on this crate — a
server, a client, a CLI, and eventually the engine itself, once
`Builder::from_spec` exists — so anything heavier is paid for by all of them.

In particular this crate must never depend on `computer`, because that edge is
going to run the other way. The name is the rule: if it is not a plain data
type, it does not go in here.
