# The pointer and the keyboard are compiled here: Debian packages no client for either protocol.
FROM debian:bookworm-slim AS pointer

RUN apt-get update && apt-get install -y --no-install-recommends \
      gcc libc6-dev libwayland-dev libwayland-bin \
  && rm -rf /var/lib/apt/lists/*

COPY pointer.c wlr-virtual-pointer-unstable-v1.xml virtual-keyboard-unstable-v1.xml /src/
RUN cd /src \
  && for protocol in wlr-virtual-pointer-unstable-v1 virtual-keyboard-unstable-v1; do \
       wayland-scanner client-header "$protocol.xml" "$protocol-client-protocol.h" \
       && wayland-scanner private-code "$protocol.xml" "$protocol-protocol.c" \
       || exit 1; \
     done \
  && gcc -O2 -Wall -Wextra -Werror -o holm-pointer \
       pointer.c wlr-virtual-pointer-unstable-v1-protocol.c \
       virtual-keyboard-unstable-v1-protocol.c -lwayland-client

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
      sway \
      wayvnc \
      grim \
      imagemagick \
      wl-clipboard \
      novnc \
      websockify \
      python3-jwcrypto \
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
  && mkdir -p /etc/holm

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
RUN useradd --create-home --shell /bin/bash --uid 1000 holm \
  && mkdir -p /tmp/holm /home/holm/.browser-profiles \
  && chown -R holm:holm /tmp/holm /home/holm

ENV HOME=/home/holm
ENV LANG=C.UTF-8
ENV HOLM_SCREEN_WIDTH=1280
ENV HOLM_SCREEN_HEIGHT=800

# Fixed here, because this is the only environment a later `docker exec` inherits.
ENV DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/holm/a11y-bus.socket
ENV QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1
WORKDIR /home/holm

COPY --from=pointer /src/holm-pointer /usr/local/bin/holm-pointer
COPY start.sh   /usr/local/bin/holm-desktop
COPY screen.sh  /usr/local/bin/holm-screen
COPY browser.sh /usr/local/bin/holm-browser
COPY input.sh   /usr/local/bin/holm-input
COPY a11y.py    /usr/local/bin/holm-a11y
COPY devtools-bridge.py /usr/local/bin/holm-devtools-bridge
COPY sway.config /etc/holm/sway.config
COPY paste.js /usr/share/novnc/app/paste.js
RUN sed -i 's|</head>|<script type="module">import UI from "./app/ui.js"; import paste from "./app/paste.js"; paste(() => UI.rfb);</script>\n</head>|' /usr/share/novnc/vnc.html \
  && grep -q app/paste.js /usr/share/novnc/vnc.html
RUN chmod +x /usr/local/bin/holm-desktop \
             /usr/local/bin/holm-screen \
             /usr/local/bin/holm-browser \
             /usr/local/bin/holm-input \
             /usr/local/bin/holm-a11y \
             /usr/local/bin/holm-devtools-bridge

LABEL holm.profile="holm-wayland"

EXPOSE 6080 6081 6082 6083 6084 6085 6086 6087 6088 6089 6090 6091 6092 6093 6094 6095

EXPOSE 9222 9223

USER holm
CMD ["/usr/local/bin/holm-desktop"]
