# A computer in a box, on Wayland.
#
# The same box as `images/desktop/`, with the display server swapped: sway
# headless in place of Xvfb and fluxbox, wayvnc in place of x11vnc, grim in
# place of ImageMagick `import`, and `computer-input` in place of xdotool.
# Everything above it — the ports, the verbs, the token protocol — is the same,
# so the crate's profile helpers need no override.
#
# Kept in step with `src/wayland.rs` by `tests/wayland_image.rs`.
# **The pointer is compiled here, not installed.** Wayland gives no client a
# way to move somebody else's pointer; synthetic input has to arrive as a
# device, through `zwlr_virtual_pointer_v1`. Debian packages no client for it,
# so this crate carries one — 200 lines of C and the protocol it speaks — and
# builds it in a stage the final image does not keep.
FROM debian:bookworm-slim AS pointer

RUN apt-get update && apt-get install -y --no-install-recommends \
      gcc libc6-dev libwayland-dev libwayland-bin \
  && rm -rf /var/lib/apt/lists/*

COPY pointer.c wlr-virtual-pointer-unstable-v1.xml /src/
RUN cd /src \
  && wayland-scanner client-header wlr-virtual-pointer-unstable-v1.xml \
       wlr-virtual-pointer-unstable-v1-client-protocol.h \
  && wayland-scanner private-code wlr-virtual-pointer-unstable-v1.xml \
       wlr-virtual-pointer-unstable-v1-protocol.c \
  && gcc -O2 -Wall -Wextra -Werror -o computer-pointer \
       pointer.c wlr-virtual-pointer-unstable-v1-protocol.c -lwayland-client

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
      sway \
      wayvnc \
      wtype \
      grim \
      wl-clipboard \
      novnc \
      websockify \
      python3 \
      chromium \
      fonts-liberation \
      fonts-dejavu-core \
      dbus-x11 \
      ca-certificates \
      procps \
      socat \
      bash \
  && rm -rf /var/lib/apt/lists/* \
  && mkdir -p /etc/computer

# Anything else the caller asked for, in its own layer so the base stays
# cached and shared.
# Repositories for packages Debian does not carry. One record per line, tab
# separated: name, key URL, deb line. Added before the install below, because a
# package from one of these does not resolve until its archive is known.
ARG EXTRA_SOURCES=""
RUN if [ -n "$EXTRA_SOURCES" ]; then \
      apt-get update \
      && apt-get install -y --no-install-recommends ca-certificates curl gnupg \
      && printf '%s\n' "$EXTRA_SOURCES" | while IFS="$(printf '\t')" read -r name key list; do \
           [ -n "$name" ] || continue; \
           curl -fsSL "$key" | gpg --dearmor -o "/usr/share/keyrings/$name.gpg"; \
           echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/$name.gpg] $list" \
             > "/etc/apt/sources.list.d/$name.list"; \
         done \
      && rm -rf /var/lib/apt/lists/*; \
    fi

ARG EXTRA_PACKAGES=""
RUN if [ -n "$EXTRA_PACKAGES" ]; then \
      apt-get update \
      && apt-get install -y --no-install-recommends $EXTRA_PACKAGES \
      && rm -rf /var/lib/apt/lists/*; \
    fi

# **Not root.** sway refuses to start as root — it cannot drop the privilege
# in a way it can prove it will not get back, so it will not take it in the
# first place. Everything in this image therefore runs as one unprivileged
# user, and `docker exec` lands as that user too.
RUN useradd --create-home --shell /bin/bash --uid 1000 computer \
  && mkdir -p /tmp/computer \
  && chown -R computer:computer /tmp/computer /home/computer

ENV HOME=/home/computer
ENV COMPUTER_SCREEN_WIDTH=1280
ENV COMPUTER_SCREEN_HEIGHT=800
WORKDIR /home/computer

COPY --from=pointer /src/computer-pointer /usr/local/bin/computer-pointer
COPY start.sh   /usr/local/bin/computer-desktop
COPY screen.sh  /usr/local/bin/computer-screen
COPY browser.sh /usr/local/bin/computer-browser
COPY input.sh   /usr/local/bin/computer-input
COPY sway.config /etc/computer/sway.config
RUN chmod +x /usr/local/bin/computer-desktop \
             /usr/local/bin/computer-screen \
             /usr/local/bin/computer-browser \
             /usr/local/bin/computer-input

# Which contract this image implements, so a box driven by another profile is
# refused before it starts rather than as a display that never came up.
LABEL computer.profile="computer-wayland"

# Eight screens, each a view port and a control port — takeover is a second
# server, never a mode switch on the first.
EXPOSE 6080 6081 6082 6083 6084 6085 6086 6087 6088 6089 6090 6091 6092 6093 6094 6095

# 9222 inside, and 9223 the bridge in front of it; see `start.sh` for why the
# bridge exists.
EXPOSE 9222 9223

USER computer
CMD ["/usr/local/bin/computer-desktop"]
