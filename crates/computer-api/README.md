# computer-api

The HTTP contract a box is managed and driven over.

Both ends depend on this and nothing else does, which is what separates it from
[`computer-types`](../computer-types): the engine needs a `Spec` and a `Point`,
and has no idea an API exists.

Every type goes both ways. The server never sends a request and never reads a
reply, so half of each pair is unused there — but a client does both, and a
protocol only one end can construct is not one.

Serde and the shared types. A client that has to compile a web framework to send
a request is a client nobody uses.
