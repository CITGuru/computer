# computer-spec

A portable description of a desktop box.

```toml
[desktop]
server   = "x11"
features = ["wide_fonts"]

[policy]
network = true
auth    = "token"
```

A spec says what desktop is wanted and nothing about where it runs, which is
what lets one travel between a container, a microVM and somebody else's cloud.
`Placement` carries the other half — runtime, memory, how long it lives — and
is kept out of the spec so two identical desktops that differ only in a memory
limit hash the same.

`Spec::digest()` names a spec by its contents, so the same desktop asked for in
two key orders is one digest.

## Nothing here knows about an image

A spec that names no size is portable across images whose natural sizes differ,
and whatever compiles it applies its own defaults and its own limits — the
`computer-desktop` image allows eight screens and a macOS guest allows one, and
neither number belongs in the description.

So `width`, `height` and `screens` are all optional, and a spec that leaves
them open is a *different* spec from one that pins them, with a different
digest. One travels to an image that has its own idea of a screen; the other
insists.

## Deliberately dependency-free

Serde and a digest. Everything that reads a spec depends on this — a server, a
client, a CLI, and eventually the engine itself, once `Builder::from_spec`
exists — so anything heavier is paid for by all of them. In particular this
crate must never depend on `computer`, because that edge is going to run the
other way.
