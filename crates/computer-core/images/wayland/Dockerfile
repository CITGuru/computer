# The pointer is compiled here: Debian packages no client for `zwlr_virtual_pointer_v1`.
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
      imagemagick \
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

# One tab-separated record per line: name, key URL, deb line.
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

# Not root: sway refuses to start as root.
RUN useradd --create-home --shell /bin/bash --uid 1000 computer \
  && mkdir -p /tmp/computer \
  && chown -R computer:computer /tmp/computer /home/computer

ENV HOME=/home/computer
ENV COMPUTER_SCREEN_WIDTH=1280
ENV COMPUTER_SCREEN_HEIGHT=800

# Fixed here, because this is the only environment a later `docker exec` inherits.
ENV DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/computer/a11y-bus.socket
ENV QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1
WORKDIR /home/computer

COPY --from=pointer /src/computer-pointer /usr/local/bin/computer-pointer
COPY start.sh   /usr/local/bin/computer-desktop
COPY screen.sh  /usr/local/bin/computer-screen
COPY browser.sh /usr/local/bin/computer-browser
COPY input.sh   /usr/local/bin/computer-input
COPY a11y.py    /usr/local/bin/computer-a11y
COPY sway.config /etc/computer/sway.config
RUN chmod +x /usr/local/bin/computer-desktop \
             /usr/local/bin/computer-screen \
             /usr/local/bin/computer-browser \
             /usr/local/bin/computer-input \
             /usr/local/bin/computer-a11y

LABEL computer.profile="computer-wayland"

EXPOSE 6080 6081 6082 6083 6084 6085 6086 6087 6088 6089 6090 6091 6092 6093 6094 6095

EXPOSE 9222 9223

USER computer
CMD ["/usr/local/bin/computer-desktop"]
