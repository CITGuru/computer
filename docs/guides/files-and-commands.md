# Files and commands

This guide runs commands in a box and moves files and text in and out of it. Commands run in the same machine as the desktop, as a shell. They do not move the pointer.

## Run a command

A command is an argument list. It does not go through a shell unless you ask for one.

```bash
holm exec "$BOX" -- ls -la /tmp
holm exec "$BOX" -- sh -c 'echo "$HOME" && uname -a'
```

The CLI prints the command's standard output and standard error, and exits with the command's exit code.

```rust
let result = computer.exec(["ls", "-la", "/tmp"]).await?;
println!("{} {}", result.code, String::from_utf8_lossy(&result.stdout));

let long = computer
    .exec_within(["sh", "-c", "sleep 200; echo done"], Duration::from_secs(300))
    .await?;
```

MCP: `run_command` with `command: ["ls", "-la", "/tmp"]`. The result gives the exit code, standard output, and standard error.

REST:

```bash
curl -X POST "$BASE/v1/boxes/$BOX/exec" \
  -H 'content-type: application/json' \
  -d '{"argv": ["ls", "-la", "/tmp"], "timeout_ms": 60000}'
```

The response is `{code, stdout, stderr, timed_out}`.

### Time limits

| Interface | Default | Change it |
| --- | --- | --- |
| Rust | 2 minutes | `exec_within(argv, duration)` |
| REST | 2 minutes | `timeout_ms`, up to 10 minutes |
| CLI | 2 minutes | No option |
| MCP `run_command` | 2 minutes | No parameter. In a `batch`, the `run_command` step takes `timeout_ms`. |

A command that reaches its limit returns `timed_out: true`. A command can exit with code 124 by itself, so check `timed_out`, not the code.

### Install tools for commands

Put the packages that your commands need in the box when you create it:

```bash
BOX=$(holm new --package jq --package ripgrep)
```

See [Configure a box](configure-a-box.md#packages).

## Move files

| Operation | CLI | MCP | Rust | REST |
| --- | --- | --- | --- | --- |
| Host file into the box | `file <box> put LOCAL [PATH]` | | `upload(from, to)` | |
| Box file to the host | `file <box> get PATH [OUT]` | | `download(from, to)` | |
| Write bytes to a file | | `write_file` (text only) | `write_file(path, bytes)` | `PUT /v1/boxes/{id}/files` |
| Read a file | `file <box> get PATH` | `read_file` (UTF-8 only) | `read_file(path)` | `GET /v1/boxes/{id}/files?path=` |

```bash
holm file "$BOX" put ./report.pdf
holm file "$BOX" put ./report.pdf /home/user/report.pdf
holm file "$BOX" get /tmp/output.csv ./output.csv
holm file "$BOX" get /etc/hostname
```

- `put` with no path writes to `/tmp/<name>`.
- `get` with no output file writes to standard output.
- The CLI `file` commands need a server. They do not work with `--local`.

```rust
computer.upload("report.pdf", "/tmp/report.pdf").await?;
computer.download("/tmp/output.csv", "output.csv").await?;

computer.write_file("/tmp/notes.txt", b"hello").await?;
let bytes = computer.read_file("/tmp/notes.txt").await?;
```

REST sends file contents as base64:

```bash
curl -X PUT "$BASE/v1/boxes/$BOX/files" \
  -H 'content-type: application/json' \
  -d "{\"path\": \"/tmp/notes.txt\", \"contents_base64\": \"$(printf hello | base64)\"}"

curl "$BASE/v1/boxes/$BOX/files?path=/tmp/notes.txt"
```

MCP `write_file` takes `text` and replaces the file. `read_file` refuses a file that is not UTF-8. It does not return bytes that only look like text. For binary files, use the CLI, Rust, or REST. In an MCP `batch`, the `write_file` step takes `contents_base64`, not `text`.

## Find files

| Operation | CLI | MCP | Rust | REST |
| --- | --- | --- | --- | --- |
| List a directory | `file <box> ls [DIR]` | `list_files` | `list_dir(path)` | `GET …/files/list?path=` |
| Search in files | `file <box> grep PATTERN DIR` | `grep` | `grep(&Search)` | `POST …/files/grep` |
| Find by name | `file <box> glob PATTERN [DIR]` | `glob` | `glob(pattern, path, limit)` | `GET …/files/glob?pattern=` |

```bash
holm file "$BOX" ls /tmp
holm file "$BOX" grep "error" /var/log --include "*.log" --ignore-case
holm file "$BOX" glob "*.png" /tmp
```

```rust
let entries = computer.list_dir("/tmp").await?;
let matches = computer
    .grep(&Search {
        pattern: "error".to_string(),
        path: "/var/log".to_string(),
        include: Some("*.log".to_string()),
        ignore_case: true,
        limit: None,
    })
    .await?;
let paths = computer.glob("*.png", "/tmp", None).await?;
```

- `grep` takes a basic regular expression and returns lines as `path:line:text`. The directory is necessary, because a search of the full filesystem returns megabytes.
- `glob` matches file names, such as `*.log`. A pattern with `/` matches the full path. The default directory is `/`.
- Both stop at 200 results. The result tells you when it stopped: `cut: true` in REST. Use `include` or a narrower directory to get fewer results.
- `list_dir` goes one level down. Sizes are in bytes.

## Use the clipboard to move text

Each screen has a clipboard. Set it to put text into the box with no typing, and read it to get text that an application copied:

```bash
holm clip "$BOX" "a long paragraph to paste"
holm keyboard "$BOX" press ctrl+v
holm clip "$BOX"
```

MCP: `clipboard` with `text` to set it, or with no `text` to read it. See [Desktop input](desktop-input.md#clipboard).

## Give a file to a web page

A page's file input opens the operating system's file chooser, which no click can fill. Put the file in the box, then give it to the input directly:

```bash
holm file "$BOX" put ./photo.jpg /tmp/photo.jpg
holm browser "$BOX" upload "Choose file" /tmp/photo.jpg --in-box
```

`holm browser upload` can also copy a host file into the box first. With no `--in-box`, the paths are on the host.

MCP: `write_file` (for a text file), then `upload_file` with the paths in the box. REST: `PUT …/files`, then an `upload` element operation.

A page can also print to a PDF in the box with `page_pdf`, and you can then read or move that file.

## Record what ran

`holmd` records each command in the box's trace, with its argument list, exit code, and whether it timed out. It records that a file write occurred, but not its bytes. See [Boxes](../concepts/boxes.md#history).
