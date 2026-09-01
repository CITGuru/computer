# computer-client

A client for `computer-server`.

```rust
let client = Client::new("http://127.0.0.1:8080").with_token(token);

let box_ = client.create(&Spec::default(), &Placement::default(), None).await?;
let result = client.act_once(&box_.id, 0, Action::OpenUrl { url }).await?;
let png = frame_png(result.frame.as_ref().unwrap())?;

client.delete(&box_.id).await?;
```

`cargo run -p computer-client --example drive` does the whole round trip:
launch, drive, read a frame, ask again with the hash you already hold and get
nothing back, set and read the clipboard, run a command, fork, read the trace,
and remove both boxes.

Every endpoint is here. REST being the complete surface is the promise the
server makes, and a client that had to reach past it for one verb would mean the
promise was not kept.

## Errors arrive as refusals

```rust
match client.act_once(&id, 9, action).await {
    Err(Error::Refused(body)) => // the server understood and said why
    Err(Error::Transport(_))  => // never reached it; `retryable()` is true
    Err(Error::Unreadable{..})=> // it answered something this client cannot read
    Ok(result) => …
}
```

`delete` sends the confirmation header for you: reaching for a method called
`delete` is the confirmation that header exists to get.
