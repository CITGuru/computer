# Ubuntu's Chromium is a Snap launcher that does not run here; Xtradeb supplies a native one.
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates \
      software-properties-common \
  && add-apt-repository -y ppa:xtradeb/apps \
  && apt-get update && apt-get install -y --no-install-recommends \
      xvfb \
      x11vnc \
      fluxbox \
      xdotool \
      wmctrl \
      x11-utils \
      x11-xserver-utils \
      xterm \
      imagemagick \
      hsetroot \
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
      xclip \
      bash \
  && rm -rf /var/lib/apt/lists/* \
  && mkdir -p /etc/computer/fluxbox \
  && printf '%s\n' '#!/bin/sh' 'exec xsetroot -solid "#111113" "$@"' > /usr/bin/fbsetbg \
  && chmod +x /usr/bin/fbsetbg \
  && ln -sfn /bin/true /usr/bin/xmessage

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

ARG EXTRA_APPS=""
RUN if [ -n "$EXTRA_APPS" ]; then \
      printf '%s\n' "$EXTRA_APPS" | while IFS="$(printf '\t')" read -r name class command; do \
        [ -n "$name" ] || continue; \
        { echo '[Desktop Entry]'; \
          echo 'Type=Application'; \
          echo "Name=$name"; \
          echo "Exec=computer-launch $class $command"; \
          echo "Icon=$(printf '%s' "$command" | cut -d" " -f1 | xargs -r basename)"; \
          echo 'Terminal=false'; \
        } > "/usr/share/applications/computer-app-$name.desktop"; \
      done; \
    fi

ENV DISPLAY=:1
ENV HOME=/home/computer
ENV COMPUTER_SCREEN_WIDTH=1280
ENV COMPUTER_SCREEN_HEIGHT=800

# Fixed here, because this is the only environment a later `docker exec` inherits.
ENV DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/computer/a11y-bus.socket
ENV QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1
WORKDIR /home/computer

COPY input-guard.sh /usr/local/bin/xdotool
COPY start.sh       /usr/local/bin/computer-desktop
COPY screen.sh      /usr/local/bin/computer-screen
COPY browser.sh     /usr/local/bin/computer-browser
COPY fluxbox.apps   /etc/computer/fluxbox/apps
COPY fluxbox.menu   /etc/computer/fluxbox/menu
COPY fluxbox.init   /etc/computer/fluxbox/init
COPY fluxbox.style  /etc/computer/fluxbox/style
COPY embed.html     /usr/share/novnc/embed.html
COPY wallpaper.sh   /usr/local/bin/computer-wallpaper
COPY launch.sh      /usr/local/bin/computer-launch
COPY tint2rc        /etc/computer/tint2rc
COPY terminal.desktop /usr/share/applications/computer-terminal.desktop
COPY browser.desktop  /usr/share/applications/computer-browser.desktop

RUN mkdir -p /usr/share/computer \
  && convert -size 128x128 xc:none \
       -fill "#15151b" -stroke "#3d3d4a" -strokewidth 4 \
       -draw "roundrectangle 8,8 120,120 26,26" \
       -stroke "#8fe3a8" -strokewidth 9 -fill none \
       -draw "polyline 42,50 64,68 42,86" \
       -draw "line 74,88 94,88" \
       /usr/share/computer/terminal.png
RUN chmod +x /usr/local/bin/computer-desktop \
             /usr/local/bin/computer-screen \
             /usr/local/bin/computer-browser \
             /usr/local/bin/xdotool \
             /usr/local/bin/computer-wallpaper \
             /usr/local/bin/computer-launch

LABEL computer.profile="computer-desktop"

EXPOSE 6080 6081 6082 6083 6084 6085 6086 6087 6088 6089 6090 6091 6092 6093 6094 6095

EXPOSE 9222 9223

CMD ["/usr/local/bin/computer-desktop"]
