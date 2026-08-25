# The X11 image contract on Ubuntu.
#
# Ubuntu ships Chromium as a Snap launcher, which does not run in this
# container. Xtradeb supplies the native package used here.
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
      x11-utils \
      x11-xserver-utils \
      xterm \
      imagemagick \
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

ARG EXTRA_PACKAGES=""
RUN if [ -n "$EXTRA_PACKAGES" ]; then \
      apt-get update \
      && apt-get install -y --no-install-recommends $EXTRA_PACKAGES \
      && rm -rf /var/lib/apt/lists/*; \
    fi

ENV DISPLAY=:1
ENV HOME=/home/computer
ENV COMPUTER_SCREEN_WIDTH=1280
ENV COMPUTER_SCREEN_HEIGHT=800
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

# The terminal's own icon, drawn rather than shipped: xterm's is the X logo,
# which reads as nothing in a dock. ImageMagick is already here for screenshots.
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

# Which contract this image implements, so a box driven by another profile is
# refused before it starts rather than as a display that never came up.
LABEL computer.profile="computer-desktop"

# Eight screens, each a view port and a control port — takeover is a second
# server, never a mode switch on the first.
EXPOSE 6080 6081 6082 6083 6084 6085 6086 6087 6088 6089 6090 6091 6092 6093 6094 6095

# 9222 inside, and 9223 the bridge in front of it; see `start.sh` for why the
# bridge exists.
EXPOSE 9222 9223

CMD ["/usr/local/bin/computer-desktop"]
