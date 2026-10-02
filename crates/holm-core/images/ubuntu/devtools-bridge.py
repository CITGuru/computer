#!/usr/bin/env python3

import asyncio
import hmac
import ipaddress
import os
import sys

LISTEN = ("0.0.0.0", 9223)
BROWSER = ("127.0.0.1", 9222)
SECRET_HEADER = b"x-computer-devtools"
LARGEST_HEAD = 64 * 1024


def chromium_accepts(host: bytes) -> bool:
    name = host.decode("latin-1").strip()
    if name.startswith("["):
        name = name[1:].split("]", 1)[0]
    else:
        name = name.rsplit(":", 1)[0] if name.count(":") == 1 else name
    if name == "localhost" or name.endswith(".localhost"):
        return True
    try:
        ipaddress.ip_address(name)
        return True
    except ValueError:
        return False


def rewritten(head: bytes, secret: bytes):
    lines = head.split(b"\r\n")
    kept = [lines[0]]
    carried = None
    length = 0
    upgrade = False
    chunked = False

    for line in lines[1:]:
        if not line:
            continue
        name, _, value = line.partition(b":")
        name = name.strip().lower()
        value = value.strip()

        if name == SECRET_HEADER:
            carried = value
            continue
        if name == b"host" and not chromium_accepts(value):
            line = b"Host: %s:%d" % (BROWSER[0].encode(), BROWSER[1])
        if name == b"content-length":
            length = int(value or b"0")
        if name == b"transfer-encoding" and b"chunked" in value.lower():
            chunked = True
        if name == b"upgrade":
            upgrade = True
        kept.append(line)

    allowed = carried is not None and hmac.compare_digest(carried, secret)
    return allowed, b"\r\n".join(kept) + b"\r\n\r\n", length, upgrade or chunked


async def pipe(reader, writer):
    try:
        while chunk := await reader.read(65536):
            writer.write(chunk)
            await writer.drain()
    except (ConnectionError, asyncio.IncompleteReadError):
        pass
    finally:
        writer.close()


async def refuse(writer, status: bytes):
    writer.write(b"HTTP/1.1 " + status + b"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    try:
        await writer.drain()
    finally:
        writer.close()


async def serve(client_reader, client_writer, secret: bytes):
    browser_writer = None
    try:
        while True:
            try:
                head = await client_reader.readuntil(b"\r\n\r\n")
            except asyncio.LimitOverrunError:
                return await refuse(client_writer, b"431 Request Header Fields Too Large")
            except (asyncio.IncompleteReadError, ConnectionError):
                return

            allowed, head, length, raw = rewritten(head, secret)
            if not allowed:
                return await refuse(client_writer, b"403 Forbidden")

            if browser_writer is None:
                browser_reader, browser_writer = await asyncio.open_connection(*BROWSER)
                asyncio.ensure_future(pipe(browser_reader, client_writer))

            browser_writer.write(head)
            if raw:
                return await pipe(client_reader, browser_writer)
            if length:
                browser_writer.write(await client_reader.readexactly(length))
            await browser_writer.drain()
    except (ConnectionError, asyncio.IncompleteReadError, ValueError):
        pass
    finally:
        if browser_writer is not None and not browser_writer.is_closing():
            browser_writer.close()


async def main():
    secret = os.environ.get("COMPUTER_DEVTOOLS_SECRET", "").encode()
    if not secret:
        sys.exit("COMPUTER_DEVTOOLS_SECRET is empty, and this bridge opens nothing without it")

    server = await asyncio.start_server(
        lambda reader, writer: serve(reader, writer, secret),
        *LISTEN,
        limit=LARGEST_HEAD,
    )
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
